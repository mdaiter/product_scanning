use std::sync::Arc;

use crate::error::Result;
use crate::traits::PriceAdvisor;
use crate::types::*;

/// No price source is wired up yet (decision: ship a stub). It returns an empty suggestion that
/// says so, instead of a made-up number. Implement `PriceAdvisor` to plug in a real source.
#[derive(uniffi::Object)]
pub struct StubPriceAdvisor;

#[uniffi::export]
impl StubPriceAdvisor {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

#[uniffi::export]
impl PriceAdvisor for StubPriceAdvisor {
    fn suggest(&self, request: PriceRequest) -> Result<PriceSuggestion> {
        let req = request;
        Ok(PriceSuggestion {
            low: String::new(),
            median: String::new(),
            high: String::new(),
            currency: req.currency,
            comps: vec![],
            note: "No price source is configured. Set a PriceAdvisor to get comparable prices.".into(),
        })
    }
}
