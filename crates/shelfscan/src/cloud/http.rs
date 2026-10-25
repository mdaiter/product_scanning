use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

use crate::error::{Result, ShelfError};

#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<Duration>,
}

impl Resp {
    pub fn json(&self) -> Result<Value> {
        serde_json::from_str(&self.body).map_err(|e| ShelfError::internal(format!("response is not JSON ({e}): {}", self.body.chars().take(200).collect::<String>())))
    }
}

#[derive(Clone, Copy)]
enum Payload<'a> {
    Get,
    Json(&'a Value),
    Form(&'a str),
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Environment switch for the privacy audit (Phase 1.11): every outgoing request is written to this directory.
pub const DUMP_ENV: &str = "SHELFSCAN_DUMP_REQUESTS";

/// Blocking HTTP with a hard timeout and bounded retries on throttling / transient server errors.
/// Errors never include request headers, so API keys and tokens cannot leak into logs or events.
pub struct Http {
    agent: ureq::Agent,
    direct: ureq::Agent,
    pub retries: u32,
    pub backoff: Duration,
    /// Request dump directory ([`DUMP_ENV`]). Headers are never written (they carry keys and tokens).
    pub dump_dir: Option<PathBuf>,
}

static DUMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Replace every base64 image (`{"type":"base64","data":...}`) with a file reference and write the image next to the dump,
/// so the audit shows exactly which pixels left the machine.
fn extract_images(v: &mut Value, dir: &Path, stem: &str, n: &mut usize) {
    match v {
        Value::Object(map) => {
            if map.get("type").and_then(|t| t.as_str()) == Some("base64") {
                if let Some(data) = map.get("data").and_then(|d| d.as_str()) {
                    let ext = match map.get("media_type").and_then(|m| m.as_str()) {
                        Some("image/png") => "png",
                        _ => "jpg",
                    };
                    let file = format!("{stem}-img{n}.{ext}");
                    let bytes = STANDARD.decode(data).unwrap_or_default();
                    let _ = std::fs::write(dir.join(&file), &bytes);
                    map.insert("data".into(), Value::String(format!("<{} bytes, see {file}>", bytes.len())));
                    *n += 1;
                    return;
                }
            }
            for x in map.values_mut() {
                extract_images(x, dir, stem, n);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| extract_images(x, dir, stem, n)),
        _ => {}
    }
}

fn dump(dir: &Path, what: &str, url: &str, body: Payload<'_>) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let stem = format!("{ms}-{:04}-{}", DUMP_SEQ.fetch_add(1, Ordering::Relaxed), what.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_"));
    let record = match body {
        Payload::Get => json!({"method": "GET", "url": url}),
        Payload::Json(b) => {
            let mut b = b.clone();
            let mut n = 0;
            extract_images(&mut b, dir, &stem, &mut n);
            json!({"method": "POST", "url": url, "images": n, "body": b})
        }
        // form bodies are OAuth requests: client secrets live there, so only the field names are kept
        Payload::Form(f) => {
            let names: Vec<&str> = f.split('&').filter_map(|kv| kv.split('=').next()).collect();
            json!({"method": "POST", "url": url, "form_fields": names, "values": "<redacted>"})
        }
    };
    let _ = std::fs::write(dir.join(format!("{stem}.json")), serde_json::to_string_pretty(&record).unwrap_or_default());
}

fn build(timeout: Duration, direct: bool) -> ureq::Agent {
    let mut b = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false);
    if direct {
        b = b.proxy(None);
    }
    b.build().into()
}

fn is_loopback(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.starts_with("127.0.0.1") || rest.starts_with("localhost") || rest.starts_with("[::1]")
}

impl Http {
    pub fn new(timeout: Duration) -> Self {
        let dump_dir = std::env::var_os(DUMP_ENV).filter(|v| !v.is_empty()).map(PathBuf::from);
        Self { agent: build(timeout, false), direct: build(timeout, true), retries: 2, backoff: Duration::from_millis(600), dump_dir }
    }

    fn agent_for(&self, url: &str) -> &ureq::Agent {
        if is_loopback(url) { &self.direct } else { &self.agent }
    }

    fn once(&self, what: &str, url: &str, headers: &[(&str, &str)], body: Payload<'_>) -> Result<Resp> {
        let agent = self.agent_for(url);
        let res = match body {
            Payload::Json(b) => {
                let mut req = agent.post(url);
                for (k, v) in headers {
                    req = req.header(*k, *v);
                }
                req.send_json(b)
            }
            Payload::Form(f) => {
                let mut req = agent.post(url).header("Content-Type", "application/x-www-form-urlencoded");
                for (k, v) in headers {
                    req = req.header(*k, *v);
                }
                req.send(f)
            }
            Payload::Get => {
                let mut req = agent.get(url);
                for (k, v) in headers {
                    req = req.header(*k, *v);
                }
                req.call()
            }
        };
        let mut resp = res.map_err(|e| ShelfError::backend(what, format!("request failed: {e}")))?;
        let status = resp.status().as_u16();
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse::<u64>().ok()).map(Duration::from_secs);
        let body = resp.body_mut().read_to_string().map_err(|e| ShelfError::backend(what, format!("cannot read response: {e}")))?;
        Ok(Resp { status, body, retry_after })
    }

    /// Send, retrying 429 / 5xx / 529 (and transport errors) with backoff. `extra_retry` lets a caller
    /// treat a 200 with a throttling error body as retryable too.
    pub fn send(&self, what: &str, url: &str, headers: &[(&str, &str)], body: Option<&Value>, extra_retry: impl Fn(&Resp) -> bool) -> Result<Resp> {
        self.send_payload(what, url, headers, body.map_or(Payload::Get, Payload::Json), extra_retry)
    }

    /// POST `application/x-www-form-urlencoded` (OAuth token endpoints). Same retry rules as [`Http::send`].
    pub fn send_form(&self, what: &str, url: &str, fields: &[(&str, &str)]) -> Result<Resp> {
        let body = fields.iter().map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v))).collect::<Vec<_>>().join("&");
        self.send_payload(what, url, &[("Accept", "application/json")], Payload::Form(&body), |_| false)
    }

    fn send_payload(&self, what: &str, url: &str, headers: &[(&str, &str)], body: Payload<'_>, extra_retry: impl Fn(&Resp) -> bool) -> Result<Resp> {
        if let Some(dir) = &self.dump_dir {
            dump(dir, what, url, body);
        }
        let mut attempt = 0;
        loop {
            let r = self.once(what, url, headers, body);
            let retry = match &r {
                Ok(resp) => matches!(resp.status, 429 | 500 | 502 | 503 | 504 | 529) || extra_retry(resp),
                Err(_) => true,
            };
            if !retry || attempt >= self.retries {
                return r;
            }
            let wait = match &r {
                Ok(resp) => resp.retry_after.map(|d| d.min(Duration::from_secs(10))),
                Err(_) => None,
            }
            .unwrap_or_else(|| self.backoff * 2u32.pow(attempt));
            std::thread::sleep(wait);
            attempt += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::mock::MockServer;

    #[test]
    fn dump_writes_bodies_and_images_but_never_headers_or_form_values() {
        let srv = MockServer::start(|_| (200, "{}".into()));
        let dir = tempfile::tempdir().unwrap();
        let mut http = Http::new(Duration::from_secs(5));
        http.dump_dir = Some(dir.path().to_path_buf());
        let jpeg = [0xFFu8, 0xD8, 0xFF, 0xE0, 1, 2, 3];
        let body = json!({"messages": [{"content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": STANDARD.encode(jpeg)}},
            {"type": "text", "text": "what is it"}]}]});
        http.send("anthropic", &format!("{}/v1/messages", srv.base), &[("x-api-key", "sk-ant-SECRET-123")], Some(&body), |_| false).unwrap();
        http.send_form("shopify-token", &format!("{}/admin/oauth/access_token", srv.base), &[("client_id", "id-1"), ("client_secret", "shpss_SECRET")]).unwrap();

        let mut files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().path()).collect();
        files.sort();
        let all: String = files.iter().filter(|p| p.extension().is_some_and(|e| e == "json")).map(|p| std::fs::read_to_string(p).unwrap()).collect();
        assert!(!all.contains("SECRET"), "a secret reached the dump: {all}");
        assert!(all.contains("what is it"));
        assert!(all.contains("\"images\": 1"));
        assert!(all.contains("client_secret") && all.contains("<redacted>"));
        let img = files.iter().find(|p| p.extension().is_some_and(|e| e == "jpg")).expect("image extracted");
        assert_eq!(std::fs::read(img).unwrap(), jpeg);
        // the request itself still carried the real image and key
        assert!(srv.recorded()[0].body.contains(&STANDARD.encode(jpeg)));
    }
}
