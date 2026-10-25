use std::sync::Arc;

use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{blocking, msg::*, Shared};
use crate::error::{Result, ShelfError};
use crate::types::*;

const CACHE_TTL_SECS: u64 = 24 * 3600;

pub struct PricingActor;

fn slug(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>()
}

/// Only runs when the user asks. Results with real comps are cached so a repeat click is free.
async fn suggest(shared: &Arc<Shared>, item_id: String, refresh: bool) -> Result<PriceSuggestion> {
    let item = shared
        .slots
        .catalog
        .ask(|reply| CatalogMsg::Item { item_id: item_id.clone(), reply })
        .await?
        .ok_or_else(|| ShelfError::invalid(format!("no such item: {item_id}")))?;
    if item.guess.title.trim().is_empty() && item.gtin.is_none() {
        return Err(ShelfError::invalid("this item has no title or barcode to price yet"));
    }
    let key = format!("price:{}:{}", shared.cfg.currency, item.gtin.clone().unwrap_or_else(|| slug(&item.guess.title)));
    if !refresh {
        if let Some(s) = shared.store.kv_get(&key)? {
            if let Ok(cached) = serde_json::from_str::<PriceSuggestion>(&s) {
                return Ok(cached);
            }
        }
    }
    let req = PriceRequest { item_id: item.id.clone(), title: item.guess.title.clone(), gtin: item.gtin.clone(), category: item.guess.category.clone(), currency: shared.cfg.currency.clone() };
    let sh = shared.clone();
    let out = blocking("price-advisor", move || sh.plugins.advisor.suggest(req)).await?;
    if !out.comps.is_empty() {
        if let Ok(json) = serde_json::to_string(&out) {
            shared.store.kv_put(&key, &json, Some(CACHE_TTL_SECS))?;
        }
    }
    Ok(out)
}

impl Actor for PricingActor {
    type Msg = PricingMsg;
    type State = Arc<Shared>;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<PricingMsg>, shared: Arc<Shared>) -> ActorResult<Arc<Shared>> {
        Ok(shared)
    }

    async fn handle(&self, _me: ActorRef<PricingMsg>, msg: PricingMsg, shared: &mut Arc<Shared>) -> ActorResult<()> {
        let PricingMsg::Suggest { item_id, refresh, reply } = msg;
        let _ = reply.send(suggest(shared, item_id, refresh).await);
        Ok(())
    }
}
