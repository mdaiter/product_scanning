//! Reference cloud plugins on the Anthropic Messages API (vision + forced tool call for structured output).
//!
//! Privacy: these types only see what the pipeline gives them, which is masked product crops
//! (`GuessRequest.image/extra_views`, `CatalogItem.images`). Keys are never put in errors.
//!
//! Status: tested against a local mock server only. UNVERIFIED against the live API.

use std::{sync::Arc, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

use super::http::Http;
use crate::error::{Result, ShelfError};
use crate::imaging::{downscale_max_side, encode_jpeg, frame_to_rgb};
use crate::traits::{ListingWriter, ProductGuesser};
use crate::types::*;

pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const MAX_IMAGE_SIDE: u32 = 1024;

struct Client {
    api_key: String,
    model: String,
    base: String,
    http: Http,
}

impl Client {
    fn new(api_key: String, model: Option<String>, base: Option<String>) -> Self {
        Self {
            api_key,
            model: model.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| DEFAULT_MODEL.into()),
            base: base.unwrap_or_else(|| DEFAULT_BASE_URL.into()).trim_end_matches('/').to_string(),
            http: Http::new(Duration::from_secs(90)),
        }
    }

    /// One request that must answer by calling `tool`; returns the tool input.
    fn call_tool(&self, system: &str, mut content: Vec<Value>, text: String, tool: &str, description: &str, schema: Value) -> Result<Value> {
        content.push(json!({"type": "text", "text": text}));
        let body = json!({
            "model": self.model,
            "max_tokens": 1500,
            "system": system,
            "messages": [{"role": "user", "content": content}],
            "tools": [{"name": tool, "description": description, "input_schema": schema}],
            "tool_choice": {"type": "tool", "name": tool},
        });
        let url = format!("{}/v1/messages", self.base);
        let headers = [("x-api-key", self.api_key.as_str()), ("anthropic-version", API_VERSION), ("content-type", "application/json")];
        let resp = self.http.send("anthropic", &url, &headers, Some(&body), |_| false)?;
        if resp.status != 200 {
            let msg = resp.json().ok().and_then(|v| v["error"]["message"].as_str().map(String::from)).unwrap_or_default();
            return Err(ShelfError::backend("anthropic", format!("HTTP {}{}", resp.status, if msg.is_empty() { String::new() } else { format!(": {msg}") })));
        }
        let v = resp.json()?;
        v["content"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["type"] == "tool_use" && b["name"] == tool))
            .map(|b| b["input"].clone())
            .ok_or_else(|| ShelfError::backend("anthropic", "the model did not call the requested tool"))
    }
}

fn image_block(jpeg: &[u8]) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": STANDARD.encode(jpeg)}})
}

fn frame_block(f: &Frame) -> Result<Value> {
    let img = downscale_max_side(frame_to_rgb(f)?, MAX_IMAGE_SIDE);
    Ok(image_block(&encode_jpeg(&img, 85)?))
}

fn str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn trim_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect::<String>().trim_end().to_string() }
}

/// Keep a tiny allow-list of formatting tags, drop attributes, scripts and everything else.
pub fn sanitize_html(input: &str) -> String {
    const ALLOWED: [&str; 9] = ["p", "ul", "ol", "li", "strong", "em", "br", "h2", "h3"];
    let mut text = input.to_string();
    for tag in ["script", "style"] {
        loop {
            let lower = text.to_lowercase();
            let Some(start) = lower.find(&format!("<{tag}")) else { break };
            let end = lower[start..].find(&format!("</{tag}>")).map(|e| start + e + tag.len() + 3).unwrap_or(text.len());
            text.replace_range(start..end.min(text.len()), "");
        }
    }
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        // `<` only starts a tag when a letter (or `/` and a letter) follows; otherwise it is plain text.
        let starts_tag = {
            let mut cs = after.chars();
            match cs.next() {
                Some(c) if c.is_ascii_alphabetic() => true,
                Some('/') => cs.next().is_some_and(|c| c.is_ascii_alphabetic()),
                _ => false,
            }
        };
        if !starts_tag {
            out.push_str("&lt;");
            rest = after;
            continue;
        }
        match after.find('>') {
            Some(gt) => {
                let inner = after[..gt].trim();
                let closing = inner.starts_with('/');
                let name: String = inner.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
                if ALLOWED.contains(&name.as_str()) {
                    match (closing, name.as_str()) {
                        (_, "br") => out.push_str("<br>"),
                        (true, n) => out.push_str(&format!("</{n}>")),
                        (false, n) => out.push_str(&format!("<{n}>")),
                    }
                }
                rest = &after[gt + 1..];
            }
            None => {
                out.push_str("&lt;");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

// ───────────────────────────── guesser ─────────────────────────────

const GUESS_SYSTEM_CATALOG: &str = "You identify retail products from photos of a single item (a crop from a store scan). Report only what you can see: give brand and product name when legible or unmistakable, otherwise a generic description such as 'Red cardboard cereal box'. Never invent a specific product, size, flavour or model you cannot see. Confidence is your certainty about the exact product; generic descriptions must be below 0.4. A person will confirm or reject your answer.";
const GUESS_SYSTEM_DESCRIBE: &str = "You write listing titles for handmade, vintage or unbranded items from photos. Give a concise, specific, search-friendly title (for example 'Black Wool Single-Breasted Blazer'), a category, and the attributes you can actually see (colour, pattern, style; material only if visible). Do not invent a brand, size or condition. Confidence reflects how well the title matches what is visible.";

/// `ProductGuesser` on the Anthropic Messages API.
#[derive(uniffi::Object)]
pub struct AnthropicGuesser {
    client: Client,
}

#[uniffi::export]
impl AnthropicGuesser {
    /// `model = None` uses `claude-sonnet-5-5`. Phones should not hold the key: point this at your own backend instead.
    #[uniffi::constructor]
    pub fn new(api_key: String, model: Option<String>) -> Arc<Self> {
        Arc::new(Self { client: Client::new(api_key, model, None) })
    }
}

impl AnthropicGuesser {
    pub fn with_base_url(api_key: &str, model: Option<String>, base: &str) -> Self {
        Self { client: Client::new(api_key.into(), model, Some(base.into())) }
    }
}

#[uniffi::export]
impl ProductGuesser for AnthropicGuesser {
    fn guess(&self, req: GuessRequest) -> Result<ProductGuess> {
        let mut content = vec![frame_block(&req.image)?];
        for f in req.extra_views.iter().take(3) {
            content.push(frame_block(f)?);
        }
        let mut text = format!("Detector label: {}.", req.detector_label);
        if !req.dominant_colors.is_empty() {
            text.push_str(&format!(" Dominant colours: {}.", req.dominant_colors.join(", ")));
        }
        if content.len() > 1 {
            text.push_str(" The extra images are other views of the same item.");
        }
        if !req.excluded_titles.is_empty() {
            text.push_str(&format!(" The person already rejected these titles, so propose something different: {}.", req.excluded_titles.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(", ")));
        }
        let system = match req.mode {
            ScanMode::BarcodeFirst => GUESS_SYSTEM_CATALOG,
            ScanMode::DescribeFirst => GUESS_SYSTEM_DESCRIBE,
        };
        let schema = json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "brand": {"type": ["string", "null"]},
                "category": {"type": ["string", "null"]},
                "attributes": {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}, "value": {"type": "string"}}, "required": ["name", "value"]}},
                "confidence": {"type": "number", "minimum": 0, "maximum": 1}
            },
            "required": ["title", "confidence"]
        });
        let v = self.client.call_tool(system, content, text, "report_product", "Report what the product is.", schema)?;
        let title = str_field(&v, "title").ok_or_else(|| ShelfError::backend("anthropic", "the answer has no title"))?;
        let attributes = v["attributes"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| Some(Attribute { name: str_field(x, "name")?, value: str_field(x, "value")? })).take(12).collect())
            .unwrap_or_default();
        let confidence = v["confidence"].as_f64().or_else(|| v["confidence"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
        Ok(ProductGuess { title: trim_chars(&title, 200), brand: str_field(&v, "brand"), category: str_field(&v, "category"), attributes, confidence })
    }
}

// ───────────────────────────── writer ─────────────────────────────

const WRITER_SYSTEM: &str = "You write Shopify product listings. Use only facts that are visible in the photos or given in the notes. For anything you cannot know (size, material, dimensions, condition, care, origin) write the literal marker [Needs your input: <what is missing>] instead of guessing. Never invent a price, a brand or a specification. Write natural, specific, search-friendly English in the requested voice. description_html may only use p, ul, ol, li, strong, em, br, h2, h3.";

/// `ListingWriter` on the Anthropic Messages API: title, description, tags and SEO text from the item's images.
#[derive(uniffi::Object)]
pub struct AnthropicWriter {
    client: Client,
}

#[uniffi::export]
impl AnthropicWriter {
    #[uniffi::constructor]
    pub fn new(api_key: String, model: Option<String>) -> Arc<Self> {
        Arc::new(Self { client: Client::new(api_key, model, None) })
    }
}

impl AnthropicWriter {
    pub fn with_base_url(api_key: &str, model: Option<String>, base: &str) -> Self {
        Self { client: Client::new(api_key.into(), model, Some(base.into())) }
    }
}

#[uniffi::export]
impl ListingWriter for AnthropicWriter {
    fn write(&self, item: CatalogItem, style_prompt: String) -> Result<Listing> {
        let content: Vec<Value> = item.images.iter().take(3).map(|j| image_block(j)).collect();
        let mut notes = vec![format!("Working title: {}", item.guess.title)];
        if let Some(b) = &item.guess.brand {
            notes.push(format!("Brand: {b}"));
        }
        if let Some(c) = &item.guess.category {
            notes.push(format!("Category: {c}"));
        }
        if let Some(g) = &item.gtin {
            notes.push(format!("Barcode (GTIN): {g}"));
        }
        for a in &item.guess.attributes {
            notes.push(format!("{}: {}", a.name, a.value));
        }
        let confirmed = matches!(item.state, ItemState::Confirmed { .. });
        let text = format!(
            "Voice: {style_prompt}\n\nNotes about the item ({}):\n- {}\n\nWrite the listing.",
            if confirmed { "confirmed by the seller" } else { "the seller has not confirmed these yet" },
            notes.join("\n- ")
        );
        let schema = json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Specific, search-friendly, at most 80 characters"},
                "description_html": {"type": "string"},
                "product_type": {"type": ["string", "null"]},
                "vendor": {"type": ["string", "null"], "description": "Only if the brand is known"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "seo_title": {"type": "string", "description": "At most 60 characters"},
                "seo_description": {"type": "string", "description": "At most 155 characters"}
            },
            "required": ["title", "description_html", "tags", "seo_title", "seo_description"]
        });
        let v = self.client.call_tool(WRITER_SYSTEM, content, text, "write_listing", "Return the finished listing.", schema)?;
        let title = str_field(&v, "title").unwrap_or_else(|| item.guess.title.clone());
        let mut html = sanitize_html(v["description_html"].as_str().unwrap_or(""));
        if html.is_empty() {
            html = format!("<p>{}</p><p><em>[Needs your input: description]</em></p>", sanitize_html(&title));
        }
        let mut tags: Vec<String> = Vec::new();
        for t in v["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str()) {
            let t = trim_chars(&t.trim().to_lowercase(), 40);
            if !t.is_empty() && !tags.contains(&t) && tags.len() < 10 {
                tags.push(t);
            }
        }
        Ok(Listing {
            id: String::new(),
            source_item_id: item.id.clone(),
            title: trim_chars(&title, 255),
            description_html: html,
            vendor: str_field(&v, "vendor").or_else(|| item.guess.brand.clone()).unwrap_or_default(),
            product_type: str_field(&v, "product_type").or_else(|| item.guess.category.clone()).unwrap_or_default(),
            tags,
            status: ListingStatus::Draft,
            options: vec![],
            variants: vec![ListingVariant { option_values: vec![], sku: item.sku.clone(), price: String::new(), compare_at_price: None, quantity: 0, barcode: item.gtin.clone() }],
            images: vec![],
            seo_title: str_field(&v, "seo_title").map(|s| trim_chars(&s, 70)),
            seo_description: str_field(&v, "seo_description").map(|s| trim_chars(&s, 160)),
            confidence: item.guess.confidence,
            published: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::mock::MockServer;
    use crate::imaging::rgb_to_frame;

    fn tool_reply(name: &str, input: Value) -> String {
        json!({"id":"msg_1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":name,"input":input}],"stop_reason":"tool_use"}).to_string()
    }

    fn crop(w: u32, h: u32) -> Frame {
        rgb_to_frame(&image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 40])), 0)
    }

    fn request(mode: ScanMode, excluded: &[&str], extra: usize) -> GuessRequest {
        GuessRequest {
            item_id: "item-001".into(),
            image: crop(300, 200),
            extra_views: (0..extra).map(|_| crop(120, 90)).collect(),
            detector_label: "product".into(),
            dominant_colors: vec!["red".into()],
            excluded_titles: excluded.iter().map(|s| s.to_string()).collect(),
            mode,
        }
    }

    #[test]
    fn guess_request_shape_privacy_and_parsing() {
        let srv = MockServer::start(|_| (200, tool_reply("report_product", json!({"title":"  Red cereal box ","brand":"Acme","category":null,"attributes":[{"name":"Color","value":"red"},{"name":"","value":"x"}],"confidence":1.7}))));
        let g = AnthropicGuesser::with_base_url("sk-test-secret", None, &srv.base);
        let out = g.guess(request(ScanMode::BarcodeFirst, &["Old title"], 2)).unwrap();
        assert_eq!(out.title, "Red cereal box");
        assert_eq!(out.brand.as_deref(), Some("Acme"));
        assert_eq!(out.category, None);
        assert_eq!(out.attributes.len(), 1, "attributes without a name are dropped");
        assert_eq!(out.confidence, 1.0, "confidence is clamped");

        let r = &srv.recorded()[0];
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/v1/messages");
        assert_eq!(r.headers["x-api-key"], "sk-test-secret");
        assert_eq!(r.headers["anthropic-version"], "2023-06-01");
        let body: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["tool_choice"], json!({"type":"tool","name":"report_product"}));
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        let images: Vec<_> = blocks.iter().filter(|b| b["type"] == "image").collect();
        assert_eq!(images.len(), 3, "the crop plus its two extra views, nothing else");
        let jpeg = STANDARD.decode(images[0]["source"]["data"].as_str().unwrap()).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (300, 200), "exactly the crop we passed in");
        let text = blocks.last().unwrap()["text"].as_str().unwrap();
        assert!(text.contains("\"Old title\"") && text.contains("red"));
        assert!(body["system"].as_str().unwrap().contains("retail products"));
    }

    #[test]
    fn describe_mode_uses_the_handmade_prompt() {
        let srv = MockServer::start(|_| (200, tool_reply("report_product", json!({"title":"Black Wool Blazer","confidence":0.7}))));
        AnthropicGuesser::with_base_url("k", Some("my-model".into()), &srv.base).guess(request(ScanMode::DescribeFirst, &[], 0)).unwrap();
        let body: Value = serde_json::from_str(&srv.recorded()[0].body).unwrap();
        assert!(body["system"].as_str().unwrap().contains("handmade"));
        assert_eq!(body["model"], "my-model");
    }

    #[test]
    fn large_crops_are_shrunk_before_upload() {
        let srv = MockServer::start(|_| (200, tool_reply("report_product", json!({"title":"x","confidence":0.1}))));
        let mut req = request(ScanMode::BarcodeFirst, &[], 0);
        req.image = crop(3000, 2000);
        AnthropicGuesser::with_base_url("k", None, &srv.base).guess(req).unwrap();
        let body: Value = serde_json::from_str(&srv.recorded()[0].body).unwrap();
        let data = body["messages"][0]["content"][0]["source"]["data"].as_str().unwrap();
        let img = image::load_from_memory(&STANDARD.decode(data).unwrap()).unwrap();
        assert!(img.width().max(img.height()) <= MAX_IMAGE_SIDE);
    }

    #[test]
    fn throttling_is_retried_and_errors_never_leak_the_key() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let srv = MockServer::start(move |_| {
            if c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                (529, r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#.into())
            } else {
                (200, tool_reply("report_product", json!({"title":"Fine","confidence":0.5})))
            }
        });
        let mut g = AnthropicGuesser::with_base_url("sk-secret-123", None, &srv.base);
        g.client.http.backoff = Duration::from_millis(5);
        assert_eq!(g.guess(request(ScanMode::BarcodeFirst, &[], 0)).unwrap().title, "Fine");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);

        let srv = MockServer::start(|_| (401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.into()));
        let g = AnthropicGuesser::with_base_url("sk-secret-123", None, &srv.base);
        let e = g.guess(request(ScanMode::BarcodeFirst, &[], 0)).unwrap_err().to_string();
        assert!(e.contains("401") && e.contains("invalid x-api-key"), "{e}");
        assert!(!e.contains("sk-secret-123"));
        assert_eq!(srv.recorded().len(), 1, "401 is not retried");
    }

    #[test]
    fn a_reply_without_the_tool_call_is_an_error() {
        let srv = MockServer::start(|_| (200, json!({"content":[{"type":"text","text":"I think it is a cereal box"}]}).to_string()));
        let e = AnthropicGuesser::with_base_url("k", None, &srv.base).guess(request(ScanMode::BarcodeFirst, &[], 0)).unwrap_err();
        assert!(matches!(e, ShelfError::Backend { .. }));
    }

    fn item() -> CatalogItem {
        CatalogItem {
            id: "item-001".into(),
            quantity: 1,
            state: ItemState::Guessed,
            role: Role::Product,
            mode: ScanMode::DescribeFirst,
            guess: ProductGuess { title: "Black wool blazer".into(), brand: None, category: Some("Jackets".into()), attributes: vec![Attribute { name: "Color".into(), value: "black".into() }], confidence: 0.6 },
            guess_ready: true,
            label: "product".into(),
            gtin: None,
            sku: Some("SKU-0001".into()),
            needs_barcode: false,
            images: vec![crate::imaging::encode_jpeg(&image::RgbImage::from_pixel(64, 64, image::Rgb([10, 10, 10])), 80).unwrap()],
            first_seen_ms: 0,
        }
    }

    #[test]
    fn writer_builds_a_clean_draft_and_never_invents_a_price() {
        let srv = MockServer::start(|_| {
            (200, tool_reply("write_listing", json!({
                "title": "Black Wool Single-Breasted Blazer",
                "description_html": "<h2 class='x' onclick='evil()'>Details</h2><p>Warm.</p><script>alert(1)</script><a href='http://spam'>click</a><ul><li>Black</li></ul><p>[Needs your input: size]</p>",
                "product_type": "Blazers", "vendor": null,
                "tags": ["Wool", "wool", "BLAZER", "  ", "black"],
                "seo_title": "Black Wool Single-Breasted Blazer for Women and Men, Warm Winter Jacket",
                "seo_description": "x".repeat(300)
            })))
        });
        let w = AnthropicWriter::with_base_url("k", None, &srv.base);
        let l = w.write(item(), "Plain, warm, no hype".into()).unwrap();
        assert_eq!(l.status, ListingStatus::Draft);
        assert_eq!(l.title, "Black Wool Single-Breasted Blazer");
        assert_eq!(l.description_html, "<h2>Details</h2><p>Warm.</p>click<ul><li>Black</li></ul><p>[Needs your input: size]</p>");
        assert_eq!(l.tags, vec!["wool", "blazer", "black"]);
        assert!(l.seo_title.unwrap().chars().count() <= 70);
        assert_eq!(l.seo_description.unwrap().chars().count(), 160);
        assert_eq!(l.product_type, "Blazers");
        assert!(l.variants[0].price.is_empty());
        assert_eq!(l.variants[0].sku.as_deref(), Some("SKU-0001"));
        let body: Value = serde_json::from_str(&srv.recorded()[0].body).unwrap();
        let text = body["messages"][0]["content"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap().to_string();
        assert!(text.contains("Plain, warm, no hype") && text.contains("has not confirmed"));
        assert!(body["system"].as_str().unwrap().contains("Needs your input"));
    }

    #[test]
    fn html_sanitizer_basics() {
        assert_eq!(sanitize_html("<p onclick=x>Hi <b>there</b></p>"), "<p>Hi there</p>");
        assert_eq!(sanitize_html("a < b and <br/> c"), "a &lt; b and <br> c");
        assert_eq!(sanitize_html("<STYLE>p{}</STYLE><P>ok</P>"), "<p>ok</p>");
        assert_eq!(sanitize_html("<script>never closed"), "");
    }
}
