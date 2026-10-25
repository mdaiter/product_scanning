//! Shopify Admin GraphQL storefront: `productSet`, always `status: DRAFT`.
//!
//! Status: request building and response handling are tested against a local mock server.
//! Checked against shopify.dev (API 2026-10): `productSet(input, synchronous)`, `ProductStatus.DRAFT`,
//! `ProductSetInput.{title,descriptionHtml,vendor,productType,tags,status,seo{title,description},productOptions,variants,files}`,
//! `ProductVariantSetInput.{sku,price,compareAtPrice,optionValues,inventoryQuantities,barcodes}` (`barcode` is deprecated),
//! `userErrors{field message code}`, scope `write_products`, max 3 options.
//! NOT run against a live shop. Images are not uploaded yet (Phase 3, see docs/phase-3-shopify-live.md).
//! Publishing the same listing twice would create a second draft, so the engine refuses to.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use crate::cloud::http::Http;
use crate::error::{Result, ShelfError};
use crate::traits::Storefront;
use crate::types::*;

pub const DEFAULT_API_VERSION: &str = "2026-10";

const MUTATION: &str = "mutation ShelfscanProductSet($input: ProductSetInput!, $synchronous: Boolean!) { productSet(input: $input, synchronous: $synchronous) { product { id handle status } userErrors { field message code } } }";

/// How the Admin API token is obtained.
enum Auth {
    /// A token you already have (for example from a legacy custom app, which still works if it exists).
    Static(String),
    /// Dev Dashboard app, `client_credentials` grant: a token valid for 24 h, fetched and refreshed here.
    /// Works only for stores in the same Shopify organization as the app.
    ClientCredentials { client_id: String, client_secret: String, cache: Mutex<Option<(String, Instant)>> },
}

#[derive(uniffi::Object)]
pub struct ShopifyStorefront {
    shop: String,
    auth: Auth,
    api_version: String,
    base: String,
    inventory_location: Option<String>,
    http: Http,
}

fn normalize_shop(s: &str) -> Result<String> {
    let s = s.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_lowercase();
    let full = if s.contains('.') { s } else { format!("{s}.myshopify.com") };
    let label = full.strip_suffix(".myshopify.com").unwrap_or("");
    let ok = !label.is_empty() && full.len() < 100 && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !label.starts_with('-');
    if ok { Ok(full) } else { Err(ShelfError::invalid("shop must look like my-store or my-store.myshopify.com")) }
}

#[uniffi::export]
impl ShopifyStorefront {
    /// Use an Admin API access token you already have (keep it in the Keychain / Keystore). Shopify stopped
    /// letting people create new "legacy custom apps" on 2026-01-01, so new setups use
    /// [`ShopifyStorefront::with_client_credentials`] or an OAuth flow in your own backend instead.
    /// `api_version = None` uses 2026-10. Pass `inventory_location_id` (a `gid://shopify/Location/...`) to also
    /// write the scanned quantity estimates as stock; leave it `None` to leave inventory alone.
    #[uniffi::constructor]
    pub fn new(shop: String, access_token: String, api_version: Option<String>, inventory_location_id: Option<String>) -> std::result::Result<Arc<Self>, ShelfError> {
        if access_token.trim().is_empty() {
            return Err(ShelfError::invalid("access token is empty"));
        }
        Ok(Arc::new(Self::build(&normalize_shop(&shop)?, Auth::Static(access_token), api_version, inventory_location_id, None)?))
    }

    /// Dev Dashboard app credentials (`client_credentials` grant). The 24-hour token is requested on first use,
    /// cached, and refreshed when it is about to expire or Shopify answers 401. The app and the store must be in
    /// the same Shopify organization. Do not ship `client_secret` inside a phone app; use a backend.
    #[uniffi::constructor]
    pub fn with_client_credentials(shop: String, client_id: String, client_secret: String, api_version: Option<String>, inventory_location_id: Option<String>) -> std::result::Result<Arc<Self>, ShelfError> {
        if client_id.trim().is_empty() || client_secret.trim().is_empty() {
            return Err(ShelfError::invalid("client id and secret are required"));
        }
        let auth = Auth::ClientCredentials { client_id, client_secret, cache: Mutex::new(None) };
        Ok(Arc::new(Self::build(&normalize_shop(&shop)?, auth, api_version, inventory_location_id, None)?))
    }
}

impl ShopifyStorefront {
    fn build(shop: &str, auth: Auth, api_version: Option<String>, loc: Option<String>, base: Option<String>) -> Result<Self> {
        Ok(Self {
            shop: shop.to_string(),
            auth,
            api_version: api_version.filter(|v| !v.trim().is_empty()).unwrap_or_else(|| DEFAULT_API_VERSION.into()),
            base: base.unwrap_or_else(|| format!("https://{shop}")),
            inventory_location: loc.filter(|l| !l.trim().is_empty()),
            http: Http::new(Duration::from_secs(60)),
        })
    }

    /// Tests only: talk to a local mock instead of `https://{shop}`.
    pub fn with_base_url(shop: &str, token: &str, base: &str) -> Result<Self> {
        Self::build(&normalize_shop(shop)?, Auth::Static(token.into()), None, None, Some(base.trim_end_matches('/').to_string()))
    }

    /// Tests only: client-credentials auth against a local mock.
    pub fn with_base_url_credentials(shop: &str, client_id: &str, client_secret: &str, base: &str) -> Result<Self> {
        let auth = Auth::ClientCredentials { client_id: client_id.into(), client_secret: client_secret.into(), cache: Mutex::new(None) };
        Self::build(&normalize_shop(shop)?, auth, None, None, Some(base.trim_end_matches('/').to_string()))
    }

    /// The token to send. For client credentials this fetches / refreshes it; `force` discards the cached one.
    fn access_token(&self, force: bool) -> std::result::Result<String, String> {
        match &self.auth {
            Auth::Static(t) => Ok(t.clone()),
            Auth::ClientCredentials { client_id, client_secret, cache } => {
                let mut g = cache.lock().map_err(|_| "token cache poisoned".to_string())?;
                if force {
                    *g = None;
                }
                if let Some((t, exp)) = g.as_ref() {
                    if Instant::now() + Duration::from_secs(300) < *exp {
                        return Ok(t.clone());
                    }
                }
                let url = format!("{}/admin/oauth/access_token", self.base);
                let resp = self
                    .http
                    .send_form("shopify-auth", &url, &[("grant_type", "client_credentials"), ("client_id", client_id), ("client_secret", client_secret)])
                    .map_err(|e| e.to_string())?;
                if resp.status != 200 {
                    // never echo the response body or the credentials
                    return Err(format!("could not get an access token (HTTP {}); check the client id, secret, and that the app and store are in the same organization", resp.status));
                }
                let v = resp.json().map_err(|e| e.to_string())?;
                let token = v["access_token"].as_str().filter(|t| !t.is_empty()).ok_or("the token response had no access_token")?.to_string();
                let secs = v["expires_in"].as_u64().unwrap_or(86_399);
                *g = Some((token.clone(), Instant::now() + Duration::from_secs(secs)));
                Ok(token)
            }
        }
    }

    fn admin_url(&self, gid: &str) -> Option<String> {
        let id = gid.rsplit('/').next().filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))?;
        let handle = self.shop.strip_suffix(".myshopify.com")?;
        Some(format!("https://admin.shopify.com/store/{handle}/products/{id}"))
    }

    /// The `ProductSetInput` for one listing. Always DRAFT, whatever the listing says.
    pub fn product_input(&self, l: &Listing) -> std::result::Result<Value, String> {
        if l.variants.is_empty() {
            return Err("listing has no variants".into());
        }
        let default_opt = l.options.is_empty();
        let options: Vec<Value> = if default_opt {
            vec![json!({"name": "Title", "values": [{"name": "Default Title"}]})]
        } else {
            l.options.iter().map(|o| json!({"name": o.name, "values": o.values.iter().map(|v| json!({"name": v})).collect::<Vec<_>>()})).collect()
        };
        let mut variants = Vec::new();
        for v in &l.variants {
            let option_values: Vec<Value> = if default_opt {
                vec![json!({"optionName": "Title", "name": "Default Title"})]
            } else {
                if v.option_values.len() != l.options.len() {
                    return Err("a variant does not have one value per option".into());
                }
                l.options.iter().zip(&v.option_values).map(|(o, val)| json!({"optionName": o.name, "name": val})).collect()
            };
            let mut var = json!({"optionValues": option_values, "price": v.price});
            if let Some(c) = &v.compare_at_price {
                var["compareAtPrice"] = json!(c);
            }
            if let Some(s) = &v.sku {
                var["sku"] = json!(s);
            }
            if let Some(b) = &v.barcode {
                // `barcode` (String) is deprecated on ProductVariantSetInput; `barcodes` replaces it.
                var["barcodes"] = json!([{ "value": b }]);
            }
            if let Some(loc) = &self.inventory_location {
                var["inventoryQuantities"] = json!([{"locationId": loc, "name": "available", "quantity": v.quantity}]);
            }
            variants.push(var);
        }
        let mut input = json!({
            "title": l.title,
            "descriptionHtml": l.description_html,
            "tags": l.tags,
            "status": "DRAFT",
            "productOptions": options,
            "variants": variants,
        });
        if !l.vendor.trim().is_empty() {
            input["vendor"] = json!(l.vendor);
        }
        if !l.product_type.trim().is_empty() {
            input["productType"] = json!(l.product_type);
        }
        if l.seo_title.is_some() || l.seo_description.is_some() {
            input["seo"] = json!({"title": l.seo_title, "description": l.seo_description});
        }
        Ok(input)
    }

    fn publish_one(&self, l: &Listing) -> PublishReceipt {
        let fail = |m: String| PublishReceipt { listing_id: l.id.clone(), ok: false, remote_id: None, url: None, message: m };
        let input = match self.product_input(l) {
            Ok(i) => i,
            Err(m) => return fail(m),
        };
        let url = format!("{}/admin/api/{}/graphql.json", self.base, self.api_version);
        let body = json!({"query": MUTATION, "variables": {"input": input, "synchronous": true}});
        let mut refreshed = false;
        let resp = loop {
            let token = match self.access_token(refreshed) {
                Ok(t) => t,
                Err(m) => return fail(m),
            };
            let headers = [("X-Shopify-Access-Token", token.as_str()), ("Content-Type", "application/json"), ("Accept", "application/json")];
            let resp = match self.http.send("shopify", &url, &headers, Some(&body), |r| r.body.contains("THROTTLED")) {
                Ok(r) => r,
                Err(e) => return fail(e.to_string()),
            };
            // an expired 24 h token: get a new one once, then try again
            if resp.status == 401 && !refreshed && matches!(self.auth, Auth::ClientCredentials { .. }) {
                refreshed = true;
                continue;
            }
            break resp;
        };
        if resp.status == 401 || resp.status == 403 {
            return fail(format!("HTTP {}: the access token was refused (check the token and its write_products scope)", resp.status));
        }
        if resp.status != 200 {
            return fail(format!("HTTP {}", resp.status));
        }
        let v = match resp.json() {
            Ok(v) => v,
            Err(e) => return fail(e.to_string()),
        };
        if let Some(errs) = v["errors"].as_array().filter(|e| !e.is_empty()) {
            let msgs: Vec<String> = errs.iter().filter_map(|e| e["message"].as_str().map(String::from)).collect();
            return fail(if msgs.is_empty() { "GraphQL error".into() } else { msgs.join("; ") });
        }
        let ps = &v["data"]["productSet"];
        if let Some(ue) = ps["userErrors"].as_array().filter(|e| !e.is_empty()) {
            let msgs: Vec<String> = ue
                .iter()
                .map(|e| {
                    let field = e["field"].as_array().map(|f| f.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(".")).unwrap_or_default();
                    let m = e["message"].as_str().unwrap_or("error");
                    if field.is_empty() { m.to_string() } else { format!("{field}: {m}") }
                })
                .collect();
            return fail(msgs.join("; "));
        }
        match ps["product"]["id"].as_str() {
            Some(id) => PublishReceipt {
                listing_id: l.id.clone(),
                ok: true,
                remote_id: Some(id.to_string()),
                url: self.admin_url(id),
                message: "created as a draft product (images are not uploaded yet)".into(),
            },
            None => fail("Shopify did not return a product id".into()),
        }
    }
}

#[uniffi::export]
impl Storefront for ShopifyStorefront {
    fn name(&self) -> String {
        format!("Shopify ({})", self.shop)
    }

    fn publish(&self, listings: Vec<Listing>) -> Result<Vec<PublishReceipt>> {
        let mut out = Vec::with_capacity(listings.len());
        let mut auth_failed: Option<String> = None;
        for l in &listings {
            if let Some(m) = &auth_failed {
                out.push(PublishReceipt { listing_id: l.id.clone(), ok: false, remote_id: None, url: None, message: m.clone() });
                continue;
            }
            let r = self.publish_one(l);
            if !r.ok && r.message.contains("access token was refused") {
                auth_failed = Some(r.message.clone());
            }
            out.push(r);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::mock::{MockServer, Recorded};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn listing(id: &str) -> Listing {
        Listing {
            id: id.into(),
            source_item_id: "item-001".into(),
            title: "Black Wool Jacket".into(),
            description_html: "<p>Warm.</p>".into(),
            vendor: "Acme".into(),
            product_type: "Jackets".into(),
            tags: vec!["wool".into()],
            status: ListingStatus::Active,
            options: vec![],
            variants: vec![ListingVariant { option_values: vec![], sku: Some("SKU-0001".into()), price: "0.00".into(), compare_at_price: None, quantity: 4, barcode: Some("4006381333931".into()) }],
            images: vec![],
            seo_title: Some("Black wool jacket".into()),
            seo_description: None,
            confidence: 0.5,
            published: false,
        }
    }

    fn ok_reply(_: &Recorded) -> (u16, String) {
        (200, json!({"data":{"productSet":{"product":{"id":"gid://shopify/Product/9876543210","handle":"black-wool-jacket","status":"DRAFT"},"userErrors":[]}}}).to_string())
    }

    fn client(srv: &MockServer) -> ShopifyStorefront {
        let mut s = ShopifyStorefront::with_base_url("my-shop", "shpat_secret", &srv.base).unwrap();
        s.http.backoff = Duration::from_millis(5);
        s
    }

    #[test]
    fn creates_a_draft_with_the_right_request_and_returns_an_admin_link() {
        let srv = MockServer::start(ok_reply);
        let rs = client(&srv).publish(vec![listing("listing-001")]).unwrap();
        assert!(rs[0].ok, "{:?}", rs[0]);
        assert_eq!(rs[0].remote_id.as_deref(), Some("gid://shopify/Product/9876543210"));
        assert_eq!(rs[0].url.as_deref(), Some("https://admin.shopify.com/store/my-shop/products/9876543210"));

        let r = &srv.recorded()[0];
        assert_eq!(r.path, "/admin/api/2026-10/graphql.json");
        assert_eq!(r.headers["x-shopify-access-token"], "shpat_secret");
        let b: Value = serde_json::from_str(&r.body).unwrap();
        assert!(b["query"].as_str().unwrap().contains("productSet"));
        assert_eq!(b["variables"]["synchronous"], true);
        let input = &b["variables"]["input"];
        assert_eq!(input["status"], "DRAFT", "an Active listing is still sent as a draft");
        assert_eq!(input["title"], "Black Wool Jacket");
        assert_eq!(input["vendor"], "Acme");
        assert_eq!(input["productOptions"][0]["name"], "Title");
        let v = &input["variants"][0];
        assert_eq!(v["optionValues"][0], json!({"optionName":"Title","name":"Default Title"}));
        assert_eq!(v["sku"], "SKU-0001");
        assert_eq!(v["barcodes"], json!([{"value": "4006381333931"}]));
        assert!(v.get("barcode").is_none(), "the deprecated field is not used");
        assert_eq!(v["price"], "0.00");
        assert!(v.get("inventoryQuantities").is_none(), "inventory is untouched unless a location was given");
    }

    #[test]
    fn inventory_is_written_only_when_a_location_is_configured() {
        let srv = MockServer::start(ok_reply);
        let s = ShopifyStorefront::build("my-shop.myshopify.com", Auth::Static("t".into()), Some("2026-07".into()), Some("gid://shopify/Location/1".into()), Some(srv.base.clone())).unwrap();
        s.publish(vec![listing("a")]).unwrap();
        let r = &srv.recorded()[0];
        assert_eq!(r.path, "/admin/api/2026-07/graphql.json");
        let b: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(b["variables"]["input"]["variants"][0]["inventoryQuantities"][0], json!({"locationId":"gid://shopify/Location/1","name":"available","quantity":4}));
    }

    #[test]
    fn multiple_options_map_to_option_values() {
        let srv = MockServer::start(ok_reply);
        let mut l = listing("m");
        l.options = vec![ListingOption { name: "Size".into(), values: vec!["S".into(), "M".into()] }];
        l.variants = ["S", "M"].iter().map(|s| ListingVariant { option_values: vec![s.to_string()], sku: None, price: "10.00".into(), compare_at_price: Some("12.00".into()), quantity: 0, barcode: None }).collect();
        let input = client(&srv).product_input(&l).unwrap();
        assert_eq!(input["productOptions"][0]["values"], json!([{"name":"S"},{"name":"M"}]));
        assert_eq!(input["variants"][1]["optionValues"][0], json!({"optionName":"Size","name":"M"}));
        assert_eq!(input["variants"][1]["compareAtPrice"], "12.00");
        l.variants[1].option_values.clear();
        assert!(client(&srv).product_input(&l).is_err());
    }

    #[test]
    fn user_errors_graphql_errors_and_bad_tokens_become_failed_receipts() {
        let srv = MockServer::start(|_| (200, json!({"data":{"productSet":{"product":null,"userErrors":[{"field":["input","variants","0","price"],"message":"Price is invalid","code":"INVALID"}]}}}).to_string()));
        let r = client(&srv).publish(vec![listing("a")]).unwrap().remove(0);
        assert!(!r.ok && r.message.contains("input.variants.0.price: Price is invalid"), "{}", r.message);

        let srv = MockServer::start(|_| (200, json!({"errors":[{"message":"Access denied for productSet field."}]}).to_string()));
        let r = client(&srv).publish(vec![listing("a")]).unwrap().remove(0);
        assert!(!r.ok && r.message.contains("Access denied"));

        // a bad token fails the first listing and short-circuits the rest, without leaking the token
        let srv = MockServer::start(|_| (401, "{}".into()));
        let rs = client(&srv).publish(vec![listing("a"), listing("b"), listing("c")]).unwrap();
        assert!(rs.iter().all(|r| !r.ok && r.message.contains("token was refused") && !r.message.contains("shpat_secret")));
        assert_eq!(srv.recorded().len(), 1);
    }

    #[test]
    fn throttling_is_retried() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let srv = MockServer::start(move |r| {
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                (200, json!({"errors":[{"message":"Throttled","extensions":{"code":"THROTTLED"}}]}).to_string())
            } else {
                ok_reply(r)
            }
        });
        let r = client(&srv).publish(vec![listing("a")]).unwrap().remove(0);
        assert!(r.ok);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    fn creds_client(srv: &MockServer) -> ShopifyStorefront {
        let mut s = ShopifyStorefront::with_base_url_credentials("my-shop", "cid", "csecret", &srv.base).unwrap();
        s.http.backoff = Duration::from_millis(5);
        s
    }

    #[test]
    fn client_credentials_fetch_one_token_cache_it_and_use_it() {
        let srv = MockServer::start(|r| {
            if r.path == "/admin/oauth/access_token" {
                (200, json!({"access_token":"tok-1","scope":"write_products","expires_in":86399}).to_string())
            } else if r.headers.get("x-shopify-access-token").map(String::as_str) == Some("tok-1") {
                ok_reply(r)
            } else {
                (401, "{}".into())
            }
        });
        let sf = creds_client(&srv);
        assert!(sf.publish(vec![listing("a")]).unwrap()[0].ok);
        assert!(sf.publish(vec![listing("b")]).unwrap()[0].ok);
        let reqs = srv.recorded();
        let tokens: Vec<_> = reqs.iter().filter(|r| r.path == "/admin/oauth/access_token").collect();
        assert_eq!(tokens.len(), 1, "the token is cached across publishes");
        assert_eq!(tokens[0].headers["content-type"], "application/x-www-form-urlencoded");
        assert_eq!(tokens[0].body, "grant_type=client_credentials&client_id=cid&client_secret=csecret");
    }

    #[test]
    fn an_expired_token_is_refreshed_once_on_401() {
        let issued = Arc::new(AtomicUsize::new(0));
        let i = issued.clone();
        let srv = MockServer::start(move |r| {
            if r.path == "/admin/oauth/access_token" {
                let n = i.fetch_add(1, Ordering::SeqCst) + 1;
                (200, json!({"access_token": format!("tok-{n}"), "expires_in": 86399}).to_string())
            } else if r.headers.get("x-shopify-access-token").map(String::as_str) == Some("tok-2") {
                ok_reply(r)
            } else {
                (401, "{}".into())
            }
        });
        let r = creds_client(&srv).publish(vec![listing("a")]).unwrap().remove(0);
        assert!(r.ok, "{}", r.message);
        assert_eq!(issued.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn credential_failures_do_not_leak_the_secret() {
        let srv = MockServer::start(|_| (400, json!({"error":"invalid_client","detail":"csecret is wrong"}).to_string()));
        let rs = creds_client(&srv).publish(vec![listing("a"), listing("b")]).unwrap();
        assert!(rs.iter().all(|r| !r.ok && r.message.contains("could not get an access token")));
        assert!(rs.iter().all(|r| !r.message.contains("csecret")));
    }

    #[test]
    fn shop_names_are_validated() {
        assert_eq!(normalize_shop("My-Shop").unwrap(), "my-shop.myshopify.com");
        assert_eq!(normalize_shop("https://my-shop.myshopify.com/").unwrap(), "my-shop.myshopify.com");
        assert!(normalize_shop("evil.example.com").is_err());
        assert!(normalize_shop("my shop").is_err());
        assert!(normalize_shop("").is_err());
        assert!(ShopifyStorefront::new("my-shop".into(), "  ".into(), None, None).is_err());
    }
}
