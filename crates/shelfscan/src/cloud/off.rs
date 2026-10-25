use std::{sync::Arc, time::Duration};

use super::http::Http;
use crate::error::{Result, ShelfError};
use crate::gtin;
use crate::traits::ProductLookup;
use crate::types::LookupResult;

pub const SOURCE: &str = "Open Food Facts (ODbL)";

/// GTIN → title/brand/category from Open Food Facts. Free, but food/drink/cosmetics only, and the
/// data is ODbL: show the attribution and share-alike any derived database. Missing products are
/// `None`, not errors.
#[derive(uniffi::Object)]
pub struct OpenFoodFactsLookup {
    base: String,
    user_agent: String,
    http: Http,
}

#[uniffi::export]
impl OpenFoodFactsLookup {
    /// `contact` goes into the User-Agent, as Open Food Facts asks (e.g. an app name and email).
    #[uniffi::constructor]
    pub fn new(contact: String) -> Arc<Self> {
        Arc::new(Self::with_base_url("https://world.openfoodfacts.org", contact))
    }
}

impl OpenFoodFactsLookup {
    pub fn with_base_url(base: &str, contact: String) -> Self {
        Self { base: base.trim_end_matches('/').to_string(), user_agent: format!("ShelfScan/0.1 ({contact})"), http: Http::new(Duration::from_secs(15)) }
    }
}

fn clean(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[uniffi::export]
impl ProductLookup for OpenFoodFactsLookup {
    fn lookup(&self, g: String) -> Result<Option<LookupResult>> {
        let g = gtin::normalize(&g).ok_or_else(|| ShelfError::invalid("not a valid GTIN"))?;
        let url = format!("{}/api/v2/product/{}.json?fields=code,product_name,brands,categories", self.base, g);
        let resp = self.http.send("open-food-facts", &url, &[("User-Agent", &self.user_agent), ("Accept", "application/json")], None, |_| false)?;
        if resp.status == 404 {
            return Ok(None);
        }
        if resp.status != 200 {
            return Err(ShelfError::backend("open-food-facts", format!("HTTP {}", resp.status)));
        }
        let v = resp.json()?;
        if v.get("status").and_then(|s| s.as_i64()) != Some(1) {
            return Ok(None);
        }
        let p = &v["product"];
        let title = clean(p.get("product_name"));
        let brand = clean(p.get("brands")).map(|b| b.split(',').next().unwrap_or("").trim().to_string()).filter(|b| !b.is_empty());
        let category = clean(p.get("categories")).and_then(|c| c.split(',').map(|x| x.trim()).rfind(|x| !x.is_empty()).map(String::from));
        if title.is_none() && brand.is_none() {
            return Ok(None);
        }
        Ok(Some(LookupResult { title, brand, category, source: SOURCE.into() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::mock::MockServer;

    #[test]
    fn parses_a_hit_sends_a_user_agent_and_validates_the_gtin_first() {
        let srv = MockServer::start(|r| {
            assert!(r.path.starts_with("/api/v2/product/3017620422003.json"), "{}", r.path);
            (200, r#"{"code":"3017620422003","status":1,"product":{"product_name":"Nutella","brands":"Ferrero, Nutella","categories":"Spreads, Sweet spreads, Hazelnut spreads"}}"#.into())
        });
        let l = OpenFoodFactsLookup::with_base_url(&srv.base, "tests@example.com".into());
        let r = l.lookup("3017620422003".into()).unwrap().unwrap();
        assert_eq!(r.title.as_deref(), Some("Nutella"));
        assert_eq!(r.brand.as_deref(), Some("Ferrero"));
        assert_eq!(r.category.as_deref(), Some("Hazelnut spreads"));
        assert_eq!(r.source, SOURCE);
        assert!(srv.recorded()[0].headers["user-agent"].contains("tests@example.com"));
        // a bad check digit never reaches the network
        assert!(matches!(l.lookup("3017620422004".into()), Err(ShelfError::InvalidInput { .. })));
        assert_eq!(srv.recorded().len(), 1);
    }

    #[test]
    fn unknown_products_are_none_and_server_errors_are_errors() {
        let srv = MockServer::start(|_| (200, r#"{"code":"4006381333931","status":0,"status_verbose":"product not found"}"#.into()));
        let l = OpenFoodFactsLookup::with_base_url(&srv.base, "x".into());
        assert!(l.lookup("4006381333931".into()).unwrap().is_none());

        let srv = MockServer::start(|_| (404, "{}".into()));
        assert!(OpenFoodFactsLookup::with_base_url(&srv.base, "x".into()).lookup("4006381333931".into()).unwrap().is_none());

        let srv = MockServer::start(|_| (403, "{}".into()));
        let mut l = OpenFoodFactsLookup::with_base_url(&srv.base, "x".into());
        l.http.retries = 0;
        assert!(matches!(l.lookup("4006381333931".into()), Err(ShelfError::Backend { .. })));
    }
}
