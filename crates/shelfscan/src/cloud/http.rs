use std::time::Duration;

use serde_json::Value;

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

/// Blocking HTTP with a hard timeout and bounded retries on throttling / transient server errors.
/// Errors never include request headers, so API keys and tokens cannot leak into logs or events.
pub struct Http {
    agent: ureq::Agent,
    direct: ureq::Agent,
    pub retries: u32,
    pub backoff: Duration,
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
        Self { agent: build(timeout, false), direct: build(timeout, true), retries: 2, backoff: Duration::from_millis(600) }
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
