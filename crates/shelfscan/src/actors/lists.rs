use std::{collections::HashMap, sync::Arc};

use super::ActorResult;
use ractor::{Actor, ActorRef};
use tokio::sync::Semaphore;

use super::{blocking, msg::*, Shared};
use crate::error::{Result, ShelfError};
use crate::types::*;

pub struct ListsActor;

pub struct ListsState {
    shared: Arc<Shared>,
    listings: HashMap<String, Listing>,
    order: Vec<String>,
    by_item: HashMap<String, String>,
    next_no: u64,
    sem: Arc<Semaphore>,
}

fn trim_to(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>().trim_end().to_string()
    }
}

/// Whatever a writer returned, the result is a complete DRAFT. Gaps are filled with visible
/// placeholders and tagged, never with invented facts.
pub fn finalize(cfg: &EngineConfig, item: &CatalogItem, mut l: Listing, id: String) -> Listing {
    l.id = id;
    l.source_item_id = item.id.clone();
    l.status = ListingStatus::Draft;
    l.published = false;
    if l.title.trim().is_empty() {
        l.title = if item.guess.title.trim().is_empty() { "Untitled product".into() } else { item.guess.title.trim().to_string() };
    }
    if l.vendor.trim().is_empty() {
        l.vendor = cfg.default_vendor.clone();
    }
    if l.variants.is_empty() {
        l.variants.push(ListingVariant { option_values: vec![], sku: None, price: String::new(), compare_at_price: None, quantity: 0, barcode: None });
    }
    let mut needs_price = false;
    for (i, v) in l.variants.iter_mut().enumerate() {
        if v.price.trim().is_empty() {
            v.price = cfg.placeholder_price.clone();
            needs_price = true;
        }
        if i == 0 {
            if v.sku.is_none() {
                v.sku = item.sku.clone();
            }
            if v.barcode.is_none() {
                v.barcode = item.gtin.clone();
            }
        }
        if v.quantity == 0 {
            v.quantity = item.quantity; // an estimate; storefronts ignore it unless told to push inventory
        }
    }
    if l.images.is_empty() {
        l.images = item.images.iter().map(|j| ListingImage { jpeg: j.clone(), alt: l.title.clone() }).collect();
    }
    let mut add = |t: &str| {
        if !l.tags.iter().any(|x| x == t) {
            l.tags.push(t.to_string());
        }
    };
    if !matches!(item.state, ItemState::Confirmed { .. }) {
        add("needs-review");
    }
    if needs_price {
        add("needs-price");
    }
    if l.seo_title.is_none() {
        l.seo_title = Some(trim_to(&l.title, 70));
    }
    l.confidence = item.guess.confidence;
    l
}

impl ListsState {
    async fn generate(&mut self, include_unconfirmed: bool) -> Result<Vec<Listing>> {
        let shared = self.shared.clone();
        let items = shared.slots.catalog.ask(|reply| CatalogMsg::Items { filter: ItemFilter::Visible, reply }).await?;
        let eligible: Vec<String> = items
            .iter()
            .filter(|i| match i.state {
                ItemState::Confirmed { .. } => true,
                ItemState::Guessed => include_unconfirmed,
                _ => false,
            })
            .map(|i| i.id.clone())
            .collect();
        if eligible.is_empty() {
            return Ok(vec![]);
        }
        let items = shared.slots.catalog.ask(|reply| CatalogMsg::AssignSkus { ids: eligible, reply }).await??;

        // Writers may be cloud calls: run a few at a time, keep the result order.
        let style = shared.cfg.style_prompt.clone();
        let handles: Vec<_> = items
            .iter()
            .cloned()
            .map(|item| {
                let (sh, sem, style) = (shared.clone(), self.sem.clone(), style.clone());
                tokio::spawn(async move {
                    let _p = sem.acquire_owned().await;
                    let it = item.clone();
                    let r = blocking("writer", move || sh.plugins.writer.write(it, style)).await;
                    (item, r)
                })
            })
            .collect();

        let mut out = Vec::new();
        for h in handles {
            let (item, res) = h.await.map_err(ShelfError::internal)?;
            match res {
                Ok(l) => {
                    let id = match self.by_item.get(&item.id) {
                        Some(id) => id.clone(),
                        None => {
                            let id = format!("listing-{:03}", self.next_no);
                            self.next_no += 1;
                            self.order.push(id.clone());
                            self.by_item.insert(item.id.clone(), id.clone());
                            id
                        }
                    };
                    let l = finalize(&shared.cfg, &item, l, id.clone());
                    shared.emit(Event::ListingDrafted { listing_id: id.clone(), title: l.title.clone() });
                    self.listings.insert(id, l.clone());
                    out.push(l);
                }
                Err(e) => shared.warn("writer", format!("{}: {e}", item.id)),
            }
        }
        Ok(out)
    }
}

impl Actor for ListsActor {
    type Msg = ListsMsg;
    type State = ListsState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<ListsMsg>, shared: Arc<Shared>) -> ActorResult<ListsState> {
        Ok(ListsState { shared, listings: HashMap::new(), order: vec![], by_item: HashMap::new(), next_no: 1, sem: Arc::new(Semaphore::new(3)) })
    }

    async fn handle(&self, _me: ActorRef<ListsMsg>, msg: ListsMsg, st: &mut ListsState) -> ActorResult<()> {
        match msg {
            ListsMsg::Generate { include_unconfirmed, reply } => {
                let _ = reply.send(st.generate(include_unconfirmed).await);
            }
            ListsMsg::Update { mut listing, reply } => {
                let r = match st.listings.get(&listing.id) {
                    Some(old) => {
                        listing.status = ListingStatus::Draft; // editing never activates a listing
                        listing.published = old.published;
                        listing.source_item_id = old.source_item_id.clone();
                        st.listings.insert(listing.id.clone(), listing.clone());
                        Ok(listing)
                    }
                    None => Err(ShelfError::invalid(format!("no such listing: {}", listing.id))),
                };
                let _ = reply.send(r);
            }
            ListsMsg::Get { ids, reply } => {
                let v = match ids {
                    None => st.order.iter().filter_map(|id| st.listings.get(id)).cloned().collect(),
                    Some(ids) => ids.iter().filter_map(|id| st.listings.get(id)).cloned().collect(),
                };
                let _ = reply.send(v);
            }
            ListsMsg::MarkPublished { receipts } => {
                for r in receipts.iter().filter(|r| r.ok) {
                    if let Some(l) = st.listings.get_mut(&r.listing_id) {
                        l.published = true;
                    }
                }
            }
            ListsMsg::Reset => {
                st.listings.clear();
                st.order.clear();
                st.by_item.clear();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::TemplateWriter;
    use crate::traits::ListingWriter;

    fn item(state: ItemState) -> CatalogItem {
        CatalogItem {
            id: "item-001".into(),
            quantity: 3,
            state,
            role: Role::Product,
            mode: ScanMode::BarcodeFirst,
            guess: ProductGuess { title: "Red box".into(), brand: None, category: None, attributes: vec![], confidence: 0.4 },
            guess_ready: true,
            label: "product".into(),
            gtin: Some("4006381333931".into()),
            sku: Some("4006381333931".into()),
            needs_barcode: false,
            images: vec![vec![1, 2, 3]],
            first_seen_ms: 0,
        }
    }

    #[test]
    fn finalize_forces_draft_fills_gaps_and_tags() {
        let cfg = EngineConfig::default();
        let it = item(ItemState::Guessed);
        let mut l = TemplateWriter.write(it.clone(), String::new()).unwrap();
        l.status = ListingStatus::Active; // a misbehaving writer
        l.vendor = String::new();
        let f = finalize(&cfg, &it, l, "listing-001".into());
        assert_eq!(f.status, ListingStatus::Draft);
        assert_eq!(f.vendor, "Unknown vendor");
        assert_eq!(f.variants[0].price, "0.00");
        assert_eq!(f.variants[0].quantity, 3);
        assert_eq!(f.variants[0].barcode.as_deref(), Some("4006381333931"));
        assert!(f.tags.contains(&"needs-review".to_string()) && f.tags.contains(&"needs-price".to_string()));
        assert_eq!(f.images.len(), 1);
        assert_eq!(f.images[0].alt, "Red box");
    }

    #[test]
    fn confirmed_items_are_not_tagged_for_review() {
        let cfg = EngineConfig::default();
        let it = item(ItemState::Confirmed { by: ConfirmedBy::Barcode });
        let l = TemplateWriter.write(it.clone(), String::new()).unwrap();
        let f = finalize(&cfg, &it, l, "listing-001".into());
        assert!(!f.tags.contains(&"needs-review".to_string()));
    }
}
