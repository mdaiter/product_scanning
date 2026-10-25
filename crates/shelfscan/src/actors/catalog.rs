//! The catalog owns every item and its state machine. It is the only writer.
//!
//! ```text
//!            guess ok                 user ✓ / typed fix
//!  Unknown ───────────▶ Guessed ──────────────────────────▶ Confirmed(User)
//!     ▲                   │  ▲                                   │
//!     │   re-guess        │  │ new guess                         │ valid GTIN, N agreeing reads
//!     └──── Rejected ◀────┘  │                                   ▼
//!             (user ✗, up to guess_retries auto re-guesses)   Confirmed(Barcode)
//! ```

use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::Ordering, Arc},
};

use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{msg::*, Shared};
use crate::backends::strategy::strategy_for;
use crate::error::{Result, ShelfError};
use crate::gtin;
use crate::imaging::{cosine, l2_normalize};
use crate::types::*;

const KV_COUNTER: &str = "item_counter";
const KV_SESSION: &str = "session_items";

struct Rec {
    item: CatalogItem,
    emb: Vec<f32>,
    emb_n: u32,
    /// (quality, jpeg), best first.
    views: Vec<(f32, Vec<u8>)>,
    colors: Vec<String>,
    rejected: Vec<String>,
    retries: u32,
    open_tracks: HashSet<u64>,
    votes: HashMap<String, u32>,
    ticket: u64,
}

impl Rec {
    fn add_view(&mut self, quality: f32, jpeg: Vec<u8>, max: usize) {
        self.views.push((quality, jpeg));
        self.views.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        self.views.truncate(max.max(1));
        self.item.images = self.views.iter().map(|v| v.1.clone()).collect();
    }
}

pub struct CatalogActor {
    /// Rebuild the current session from the store (used when the supervisor restarts this stage).
    pub restore: bool,
}

pub struct CatalogState {
    shared: Arc<Shared>,
    recs: HashMap<String, Rec>,
    order: Vec<String>,
    track_item: HashMap<u64, String>,
    next_no: u64,
    default_mode: ScanMode,
    pending: u32,
    epoch: u32,
    waiters: Vec<Barrier>,
}

fn overlay_item(item: &CatalogItem) -> OverlayItem {
    let label = match item.state {
        ItemState::Unknown => "Identifying…".to_string(),
        ItemState::Guessed => strategy_for(item.mode).confirm_prompt(item),
        ItemState::Confirmed { .. } => format!("✓ {}", item.guess.title),
        ItemState::Rejected => "Rejected — type a name or wait for a new guess".to_string(),
    };
    OverlayItem { item_id: item.id.clone(), state: item.state, role: item.role, label, needs_barcode: item.needs_barcode }
}

fn err_item(id: &str) -> ShelfError {
    ShelfError::invalid(format!("no such item: {id}"))
}

impl CatalogState {
    // ───────────── bookkeeping ─────────────

    fn summary(&self) -> ScanSummary {
        let mut s = self.shared.stats.snapshot();
        s.items = self.recs.values().filter(|r| r.item.role == Role::Product).count() as u32;
        s
    }

    fn sync_stats(&self) {
        let n = self.recs.values().filter(|r| r.item.role == Role::Product).count() as u64;
        self.shared.stats.items.store(n, Ordering::Relaxed);
    }

    fn persist(&self, id: &str) {
        if let Some(r) = self.recs.get(id) {
            if let Err(e) = self.shared.store.save_item(&r.item, &r.emb) {
                self.shared.warn("store", e);
            }
        }
    }

    fn save_session(&self) {
        if let Ok(json) = serde_json::to_string(&self.order) {
            if let Err(e) = self.shared.store.kv_put(KV_SESSION, &json, None) {
                self.shared.warn("store", e);
            }
        }
    }

    fn notify_overlay(&self, id: &str) {
        if let Some(r) = self.recs.get(id) {
            self.shared.slots.overlay.cast(OverlayMsg::Item(overlay_item(&r.item)));
        }
    }

    fn set_state(&mut self, id: &str, state: ItemState) {
        if let Some(r) = self.recs.get_mut(id) {
            if r.item.state != state {
                r.item.state = state;
                self.shared.emit(Event::ItemStateChanged { item_id: id.to_string(), state });
            }
        }
    }

    fn finish_barriers_if_idle(&mut self) {
        if self.pending > 0 || self.waiters.is_empty() {
            return;
        }
        for b in std::mem::take(&mut self.waiters) {
            let summary = self.summary();
            self.shared.emit(Event::ScanFlushed { items: summary.items });
            if b.kind == BarrierKind::Reset {
                self.reset_session();
            }
            let _ = b.reply.send(summary);
        }
    }

    fn reset_session(&mut self) {
        self.recs.clear();
        self.order.clear();
        self.track_item.clear();
        self.epoch += 1;
        self.pending = 0;
        self.shared.set_focus(None);
        self.shared.slots.overlay.cast(OverlayMsg::Reset);
        self.shared.slots.lists.cast(ListsMsg::Reset);
        self.shared.stats.reset();
        self.save_session();
    }

    fn insert_loaded(&mut self, item: CatalogItem, emb: Vec<f32>) {
        let id = item.id.clone();
        let views = item.images.iter().map(|j| (0.0, j.clone())).collect();
        self.recs.insert(
            id.clone(),
            Rec { item, emb, emb_n: 1, views, colors: vec![], rejected: vec![], retries: 0, open_tracks: HashSet::new(), votes: HashMap::new(), ticket: 1 },
        );
        if !self.order.contains(&id) {
            self.order.push(id.clone());
        }
        self.notify_overlay(&id);
    }

    // ───────────── guessing & lookup ─────────────

    fn dispatch_guess(&mut self, id: &str) {
        let Some(rec) = self.recs.get_mut(id) else { return };
        rec.ticket += 1;
        let strat = strategy_for(rec.item.mode);
        let job = GuessJob {
            item_id: id.to_string(),
            ticket: rec.ticket,
            epoch: self.epoch,
            jpegs: rec.views.iter().take(1 + strat.extra_views()).map(|v| v.1.clone()).collect(),
            label: rec.item.label.clone(),
            colors: rec.colors.clone(),
            excluded: rec.rejected.clone(),
            mode: rec.item.mode,
        };
        self.pending += 1;
        if !self.shared.slots.guess.cast(GuessMsg::Guess(job)) {
            self.pending -= 1;
            self.shared.warn("guess", "guess stage is not running");
        }
    }

    fn dispatch_lookup(&mut self, id: &str, gtin: &str) {
        if self.shared.plugins.lookup.is_none() {
            return;
        }
        self.pending += 1;
        if !self.shared.slots.lookup.cast(LookupMsg::Lookup { item_id: id.to_string(), gtin: gtin.to_string(), epoch: self.epoch }) {
            self.pending -= 1;
        }
    }

    fn on_guess_done(&mut self, item_id: String, ticket: u64, epoch: u32, result: Result<ProductGuess>) {
        if epoch != self.epoch {
            return;
        }
        self.pending = self.pending.saturating_sub(1);
        self.apply_guess(&item_id, ticket, result);
        self.finish_barriers_if_idle();
    }

    fn apply_guess(&mut self, id: &str, ticket: u64, result: Result<ProductGuess>) {
        let max_retries = self.shared.cfg.guess_retries;
        let shared = self.shared.clone();
        let Some(rec) = self.recs.get_mut(id) else { return };
        if ticket != rec.ticket || matches!(rec.item.state, ItemState::Confirmed { .. }) {
            return; // stale, or the user/barcode already decided
        }
        let guess = match result {
            Ok(g) => g,
            Err(e) => {
                shared.warn("guess", format!("{id}: {e}"));
                return;
            }
        };
        if rec.rejected.iter().any(|r| r.eq_ignore_ascii_case(guess.title.trim())) {
            // The guesser ignored `excluded_titles`. Try again while we have retries, then give up.
            if rec.retries < max_retries {
                rec.retries += 1;
                self.dispatch_guess(id);
            } else {
                shared.warn("guess", format!("{id}: guesser keeps proposing a rejected title; type a name instead"));
            }
            return;
        }
        let strat = strategy_for(rec.item.mode);
        rec.item.guess = guess;
        rec.item.guess_ready = true;
        rec.item.needs_barcode = strat.wants_barcode() && rec.item.gtin.is_none();
        let (title, confidence, hint, needs) = (rec.item.guess.title.clone(), rec.item.guess.confidence, strat.barcode_hint(&rec.item), rec.item.needs_barcode);
        shared.emit(Event::GuessReady { item_id: id.to_string(), title, confidence });
        self.set_state(id, ItemState::Guessed);
        if needs {
            shared.emit(Event::BarcodeRequested { item_id: id.to_string(), hint });
        }
        self.notify_overlay(id);
        self.persist(id);
    }

    fn on_lookup_done(&mut self, item_id: String, gtin: String, epoch: u32, result: Result<Option<LookupResult>>) {
        if epoch != self.epoch {
            return;
        }
        self.pending = self.pending.saturating_sub(1);
        let shared = self.shared.clone();
        if let Some(rec) = self.recs.get_mut(&item_id) {
            if rec.item.gtin.as_deref() == Some(gtin.as_str()) {
                match result {
                    Ok(Some(r)) => {
                        let user_titled = rec.item.state == (ItemState::Confirmed { by: ConfirmedBy::User });
                        if let (Some(t), false) = (r.title, user_titled) {
                            rec.item.guess.title = t;
                        }
                        if rec.item.guess.brand.is_none() {
                            rec.item.guess.brand = r.brand;
                        }
                        if rec.item.guess.category.is_none() {
                            rec.item.guess.category = r.category;
                        }
                        rec.item.guess.attributes.retain(|a| a.name != "Source");
                        rec.item.guess.attributes.push(Attribute { name: "Source".into(), value: r.source });
                        rec.item.guess.confidence = 1.0;
                        rec.item.guess_ready = true;
                        shared.emit(Event::GuessReady { item_id: item_id.clone(), title: rec.item.guess.title.clone(), confidence: 1.0 });
                        shared.emit(Event::ItemUpdated { item_id: item_id.clone(), quantity: rec.item.quantity });
                        self.notify_overlay(&item_id);
                        self.persist(&item_id);
                    }
                    Ok(None) => shared.warn("lookup", format!("{item_id}: GTIN {gtin} not found; keeping the visual guess, please check the title")),
                    Err(e) => shared.warn("lookup", format!("{item_id}: {e}")),
                }
            }
        }
        self.finish_barriers_if_idle();
    }

    // ───────────────────────────── observations ─────────────────────────────

    fn observe(&mut self, o: Observation) {
        let shared = self.shared.clone();
        let cfg = &shared.cfg;
        let max_views = cfg.max_images_per_item as usize;

        // 1. A track we already know: another view of the same item.
        if let Some(id) = self.track_item.get(&o.track_id).cloned() {
            if let Some(rec) = self.recs.get_mut(&id) {
                rec.add_view(o.quality, o.jpeg, max_views);
                self.persist(&id);
            }
            return;
        }

        // 2. Capture session: the user flipped the focused item, so the central track belongs to it.
        if o.focus_bind {
            if let Some(fid) = shared.focus() {
                if let Some(rec) = self.recs.get_mut(&fid) {
                    rec.open_tracks.insert(o.track_id);
                    rec.add_view(o.quality, o.jpeg, max_views);
                    self.track_item.insert(o.track_id, fid.clone());
                    shared.slots.overlay.cast(OverlayMsg::Bind { track_id: o.track_id, item_id: fid.clone() });
                    self.persist(&fid);
                    return;
                }
            }
        }

        // 3. Same SKU as something we have? (appearance only; a shared GTIN merges later)
        let best = self
            .recs
            .iter()
            .map(|(id, r)| (id.clone(), cosine(&r.emb, &o.embedding)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((id, sim)) = best {
            if sim >= cfg.sku_similarity {
                let mut grew = None;
                if let Some(rec) = self.recs.get_mut(&id) {
                    rec.open_tracks.insert(o.track_id);
                    let n = rec.emb_n as f32;
                    for (a, b) in rec.emb.iter_mut().zip(&o.embedding) {
                        *a = (*a * n + *b) / (n + 1.0);
                    }
                    l2_normalize(&mut rec.emb);
                    rec.emb_n += 1;
                    rec.add_view(o.quality, o.jpeg, max_views);
                    // Quantity = most units of this product seen at the same time.
                    let concurrent = rec.open_tracks.len() as u32;
                    if concurrent > rec.item.quantity {
                        rec.item.quantity = concurrent;
                        grew = Some((rec.item.quantity, rec.item.role));
                    }
                }
                self.track_item.insert(o.track_id, id.clone());
                shared.slots.overlay.cast(OverlayMsg::Bind { track_id: o.track_id, item_id: id.clone() });
                if let Some((q, role)) = grew {
                    if role == Role::Product {
                        shared.emit(Event::ItemUpdated { item_id: id.clone(), quantity: q });
                    }
                }
                self.persist(&id);
                return;
            }
        }

        // 4. A new item.
        let no = self.next_no;
        self.next_no += 1;
        if let Err(e) = shared.store.kv_put(KV_COUNTER, &self.next_no.to_string(), None) {
            shared.warn("store", e);
        }
        let id = format!("item-{no:03}");
        let mut role = o.role;
        if role == Role::Product {
            if let Ok(Some(_)) = shared.store.nearest_fixture(&o.embedding, cfg.fixture_similarity) {
                role = Role::Excluded; // the user hid this exact thing before
            }
        }
        if let Ok(near) = shared.store.nearest_items(&o.embedding, 3) {
            if let Some((eid, sim)) = near.into_iter().find(|(eid, sim)| !self.recs.contains_key(eid) && *sim >= cfg.duplicate_similarity) {
                shared.emit(Event::PossibleDuplicate { item_id: id.clone(), existing_id: eid, similarity: sim });
            }
        }
        let item = CatalogItem {
            id: id.clone(),
            quantity: 1,
            state: ItemState::Unknown,
            role,
            mode: self.default_mode,
            guess: ProductGuess { title: String::new(), brand: None, category: None, attributes: vec![], confidence: 0.0 },
            guess_ready: false,
            label: o.label,
            gtin: None,
            sku: None,
            needs_barcode: false,
            images: vec![o.jpeg.clone()],
            first_seen_ms: o.ts_ms,
        };
        let mut open = HashSet::new();
        open.insert(o.track_id);
        self.recs.insert(
            id.clone(),
            Rec {
                item,
                emb: o.embedding,
                emb_n: 1,
                views: vec![(o.quality, o.jpeg)],
                colors: o.colors,
                rejected: vec![],
                retries: 0,
                open_tracks: open,
                votes: HashMap::new(),
                ticket: 0,
            },
        );
        self.order.push(id.clone());
        self.track_item.insert(o.track_id, id.clone());
        self.save_session();
        self.persist(&id);
        self.sync_stats();
        shared.slots.overlay.cast(OverlayMsg::Bind { track_id: o.track_id, item_id: id.clone() });
        self.notify_overlay(&id);
        if role == Role::Product {
            shared.emit(Event::ItemDiscovered { item_id: id.clone(), quantity: 1 });
            self.dispatch_guess(&id);
        }
    }

    fn on_track_closed(&mut self, track_id: u64) {
        if let Some(id) = self.track_item.get(&track_id) {
            if let Some(rec) = self.recs.get_mut(id) {
                rec.open_tracks.remove(&track_id);
            }
        }
    }

    // ───────────────────────────── barcodes ─────────────────────────────

    fn on_barcode_reads(&mut self, item_id: String, reads: Vec<BarcodeRead>) {
        let votes_needed = self.shared.cfg.barcode_votes.max(1);
        // The user may have moved on; only the focused item takes reads.
        if self.shared.focus().as_deref() != Some(item_id.as_str()) {
            return;
        }
        let Some(rec) = self.recs.get_mut(&item_id) else { return };
        if rec.item.gtin.is_some() {
            return;
        }
        let distinct: HashSet<String> = reads.iter().filter_map(|r| gtin::normalize(&r.text)).collect();
        let mut winner = None;
        for g in distinct {
            let v = rec.votes.entry(g.clone()).or_insert(0);
            *v += 1;
            if *v >= votes_needed {
                winner = Some(g);
                break;
            }
        }
        if let Some(g) = winner {
            let _ = self.apply_gtin(&item_id, g);
        }
    }

    /// Records a validated GTIN. Returns the id of the item that now carries it: `id` itself, or an
    /// older item with the same GTIN that `id` was merged into.
    fn apply_gtin(&mut self, id: &str, g: String) -> String {
        let canon = gtin::canonical(&g);
        let other = self
            .recs
            .iter()
            .find(|(oid, r)| oid.as_str() != id && r.item.gtin.as_deref().and_then(gtin::canonical) == canon)
            .map(|(oid, _)| oid.clone());
        if let Some(other) = other {
            // Same GTIN = same product, whatever the pictures looked like. Keep the older item.
            let pos = |x: &str| self.order.iter().position(|o| o == x).unwrap_or(usize::MAX);
            let (from, into) = if pos(id) < pos(&other) { (other, id.to_string()) } else { (id.to_string(), other) };
            if from == id {
                // This item carries no GTIN yet; give it to the survivor's view of the world.
            } else if let Some(r) = self.recs.get_mut(id) {
                r.item.gtin = Some(g.clone());
            }
            self.merge(&from, &into);
            return into;
        }
        let shared = self.shared.clone();
        if let Some(rec) = self.recs.get_mut(id) {
            rec.item.gtin = Some(g.clone());
            rec.item.needs_barcode = false;
            rec.votes.clear();
        }
        shared.emit(Event::BarcodeRead { item_id: id.to_string(), gtin: g.clone() });
        self.set_state(id, ItemState::Confirmed { by: ConfirmedBy::Barcode });
        self.notify_overlay(id);
        self.persist(id);
        self.dispatch_lookup(id, &g);
        id.to_string()
    }

    fn merge(&mut self, from: &str, into: &str) {
        let Some(src) = self.recs.remove(from) else { return };
        self.order.retain(|o| o != from);
        let max_views = self.shared.cfg.max_images_per_item as usize;
        if let Some(dst) = self.recs.get_mut(into) {
            dst.open_tracks.extend(src.open_tracks.iter().copied());
            dst.item.quantity = dst.item.quantity.max(src.item.quantity).max(dst.open_tracks.len() as u32);
            for (q, j) in src.views {
                dst.add_view(q, j, max_views);
            }
            if dst.item.gtin.is_none() {
                dst.item.gtin = src.item.gtin;
            }
        }
        for v in self.track_item.values_mut() {
            if v == from {
                *v = into.to_string();
            }
        }
        if self.shared.focus().as_deref() == Some(from) {
            self.shared.set_focus(Some(into.to_string()));
        }
        self.shared.slots.overlay.cast(OverlayMsg::Rebind { from: from.to_string(), into: into.to_string() });
        if let Err(e) = self.shared.store.delete_item(from) {
            self.shared.warn("store", e);
        }
        self.shared.emit(Event::ItemMerged { from_id: from.to_string(), into_id: into.to_string() });
        self.save_session();
        self.sync_stats();
        self.notify_overlay(into);
        self.persist(into);
    }

    // ───────────────────────────── commands ─────────────────────────────

    fn snapshot(&self, id: &str) -> Result<CatalogItem> {
        self.recs.get(id).map(|r| r.item.clone()).ok_or_else(|| err_item(id))
    }

    fn confirm(&mut self, id: &str) -> Result<CatalogItem> {
        let rec = self.recs.get_mut(id).ok_or_else(|| err_item(id))?;
        if !rec.item.guess_ready {
            return Err(ShelfError::invalid("nothing to confirm yet: no guess for this item"));
        }
        if matches!(rec.item.state, ItemState::Confirmed { .. }) {
            return self.snapshot(id);
        }
        rec.item.needs_barcode = false;
        self.set_state(id, ItemState::Confirmed { by: ConfirmedBy::User });
        self.notify_overlay(id);
        self.persist(id);
        self.snapshot(id)
    }

    fn reject(&mut self, id: &str, correction: Option<String>) -> Result<CatalogItem> {
        let max_retries = self.shared.cfg.guess_retries;
        let shared = self.shared.clone();
        let rec = self.recs.get_mut(id).ok_or_else(|| err_item(id))?;
        let typed = correction.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
        if let Some(title) = typed {
            // A typed correction is the user's decision.
            rec.item.guess = ProductGuess { title, brand: rec.item.guess.brand.clone(), category: rec.item.guess.category.clone(), attributes: vec![], confidence: 1.0 };
            rec.item.guess_ready = true;
            rec.item.needs_barcode = false;
            rec.ticket += 1; // any guess still in flight is now stale
            let keep_barcode = matches!(rec.item.state, ItemState::Confirmed { by: ConfirmedBy::Barcode });
            if !keep_barcode {
                self.set_state(id, ItemState::Confirmed { by: ConfirmedBy::User });
            }
            shared.emit(Event::ItemUpdated { item_id: id.to_string(), quantity: self.recs[id].item.quantity });
            self.notify_overlay(id);
            self.persist(id);
            return self.snapshot(id);
        }
        match rec.item.state {
            ItemState::Confirmed { by: ConfirmedBy::Barcode } => {
                return Err(ShelfError::invalid("this item was identified by its barcode; type a correction instead"));
            }
            ItemState::Unknown if rec.ticket == 0 || !rec.item.guess_ready => {
                // Nothing was ever guessed (e.g. the guesser failed): just ask again.
                self.dispatch_guess(id);
                return self.snapshot(id);
            }
            _ => {}
        }
        if !rec.item.guess.title.is_empty() {
            rec.rejected.push(rec.item.guess.title.clone());
        }
        rec.item.guess_ready = false;
        let retry = rec.retries < max_retries;
        if retry {
            rec.retries += 1;
        }
        self.set_state(id, ItemState::Rejected);
        self.notify_overlay(id);
        if retry {
            self.dispatch_guess(id);
        } else {
            shared.warn("guess", format!("{id}: automatic re-guesses used up; type a name for it"));
        }
        self.persist(id);
        self.snapshot(id)
    }

    fn set_role(&mut self, id: &str, role: Role, similar: bool) -> Result<CatalogItem> {
        let sim_thr = self.shared.cfg.sku_similarity;
        let shared = self.shared.clone();
        let (emb, label, prev_role) = {
            let rec = self.recs.get_mut(id).ok_or_else(|| err_item(id))?;
            let prev = rec.item.role;
            rec.item.role = role;
            (rec.emb.clone(), if rec.item.guess.title.is_empty() { rec.item.label.clone() } else { rec.item.guess.title.clone() }, prev)
        };
        let mut changed = vec![id.to_string()];
        if role == Role::Excluded && prev_role != Role::Excluded {
            if let Err(e) = shared.store.add_fixture(&label, &emb) {
                shared.warn("store", e);
            }
        }
        if similar && role != Role::Product {
            let twins: Vec<String> = self.recs.iter().filter(|(oid, r)| oid.as_str() != id && r.item.role != role && cosine(&r.emb, &emb) >= sim_thr).map(|(oid, _)| oid.clone()).collect();
            for t in twins {
                if let Some(r) = self.recs.get_mut(&t) {
                    r.item.role = role;
                }
                changed.push(t);
            }
        }
        if role != Role::Product && shared.focus().map(|f| changed.contains(&f)).unwrap_or(false) {
            shared.set_focus(None);
            shared.slots.overlay.cast(OverlayMsg::Focus(None));
        }
        for cid in &changed {
            let needs_guess = self.recs.get(cid).map(|r| role == Role::Product && r.ticket == 0).unwrap_or(false);
            if needs_guess {
                self.dispatch_guess(cid);
            }
            if let Some(r) = self.recs.get(cid) {
                shared.emit(Event::ItemUpdated { item_id: cid.clone(), quantity: r.item.quantity });
            }
            self.notify_overlay(cid);
            self.persist(cid);
        }
        self.sync_stats();
        self.snapshot(id)
    }

    fn focus(&mut self, id: Option<String>) -> Result<()> {
        if let Some(i) = &id {
            let rec = self.recs.get(i).ok_or_else(|| err_item(i))?;
            if rec.item.role != Role::Product {
                return Err(ShelfError::invalid("only a visible product can be focused"));
            }
        }
        if let Some(rec) = id.as_ref().and_then(|i| self.recs.get_mut(i)) {
            rec.votes.clear();
        }
        self.shared.set_focus(id.clone());
        self.shared.slots.overlay.cast(OverlayMsg::Focus(id));
        Ok(())
    }

    fn submit_barcode(&mut self, id: &str, text: &str) -> Result<CatalogItem> {
        let g = gtin::normalize(text).ok_or_else(|| ShelfError::invalid(format!("`{text}` is not a valid barcode (UPC/EAN length or check digit is wrong)")))?;
        if !self.recs.contains_key(id) {
            return Err(err_item(id));
        }
        // The item may have been merged into an older twin with the same GTIN; return the survivor.
        let survivor = self.apply_gtin(id, g);
        self.snapshot(&survivor)
    }

    fn skip_barcode(&mut self, id: &str) -> Result<CatalogItem> {
        let rec = self.recs.get_mut(id).ok_or_else(|| err_item(id))?;
        rec.item.needs_barcode = false;
        self.notify_overlay(id);
        self.persist(id);
        self.snapshot(id)
    }

    fn set_mode(&mut self, id: Option<String>, mode: ScanMode) -> Result<()> {
        let Some(id) = id else {
            self.default_mode = mode;
            return Ok(());
        };
        let rec = self.recs.get_mut(&id).ok_or_else(|| err_item(&id))?;
        if rec.item.mode == mode {
            return Ok(());
        }
        rec.item.mode = mode;
        let strat = strategy_for(mode);
        let confirmed = matches!(rec.item.state, ItemState::Confirmed { .. });
        rec.item.needs_barcode = !confirmed && rec.item.guess_ready && strat.wants_barcode() && rec.item.gtin.is_none();
        let regenerate = !confirmed && rec.ticket > 0 && rec.item.role == Role::Product;
        self.notify_overlay(&id);
        self.persist(&id);
        if regenerate {
            self.dispatch_guess(&id);
        }
        Ok(())
    }

    fn items(&self, filter: ItemFilter) -> Vec<CatalogItem> {
        self.order
            .iter()
            .filter_map(|id| self.recs.get(id))
            .filter(|r| match filter {
                ItemFilter::All => true,
                ItemFilter::Visible => r.item.role == Role::Product,
                ItemFilter::NeedsReview => r.item.role == Role::Product && matches!(r.item.state, ItemState::Guessed | ItemState::Rejected),
            })
            .map(|r| r.item.clone())
            .collect()
    }

    fn assign_skus(&mut self, ids: Vec<String>) -> Result<Vec<CatalogItem>> {
        let prefix = self.shared.cfg.sku_prefix.clone();
        let mut out = Vec::new();
        for id in ids {
            let rec = self.recs.get_mut(&id).ok_or_else(|| err_item(&id))?;
            if rec.item.sku.is_none() {
                rec.item.sku = Some(match &rec.item.gtin {
                    Some(g) => g.clone(),
                    None => self.shared.store.next_sku(&prefix)?,
                });
                self.persist(&id);
            }
            out.push(self.snapshot(&id)?);
        }
        Ok(out)
    }

    fn resume(&mut self) -> Result<u32> {
        let mut n = 0;
        for (item, emb) in self.shared.store.load_items()? {
            if !self.recs.contains_key(&item.id) {
                self.insert_loaded(item, emb);
                n += 1;
            }
        }
        self.save_session();
        self.sync_stats();
        Ok(n)
    }
}

impl Actor for CatalogActor {
    type Msg = CatalogMsg;
    type State = CatalogState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<CatalogMsg>, shared: Arc<Shared>) -> ActorResult<CatalogState> {
        let next_no = shared.store.kv_get(KV_COUNTER)?.and_then(|s| s.parse().ok()).unwrap_or(1);
        let mut st = CatalogState {
            default_mode: shared.cfg.default_mode,
            shared: shared.clone(),
            recs: HashMap::new(),
            order: vec![],
            track_item: HashMap::new(),
            next_no,
            pending: 0,
            epoch: 0,
            waiters: vec![],
        };
        if self.restore {
            let ids: Vec<String> = shared.store.kv_get(KV_SESSION)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            for (item, emb) in shared.store.load_items()? {
                if ids.contains(&item.id) {
                    st.insert_loaded(item, emb);
                }
            }
            st.order.sort_by_key(|id| ids.iter().position(|x| x == id).unwrap_or(usize::MAX));
            st.sync_stats();
            shared.warn("catalog", format!("restarted; restored {} items from the store", st.recs.len()));
        }
        Ok(st)
    }

    async fn handle(&self, _me: ActorRef<CatalogMsg>, msg: CatalogMsg, st: &mut CatalogState) -> ActorResult<()> {
        match msg {
            CatalogMsg::Observe(o) => st.observe(o),
            CatalogMsg::TrackClosed(t) => st.on_track_closed(t),
            CatalogMsg::GuessDone { item_id, ticket, epoch, result } => st.on_guess_done(item_id, ticket, epoch, result),
            CatalogMsg::LookupDone { item_id, gtin, epoch, result } => st.on_lookup_done(item_id, gtin, epoch, result),
            CatalogMsg::BarcodeReads { item_id, reads } => st.on_barcode_reads(item_id, reads),
            CatalogMsg::Barrier(b) => {
                st.waiters.push(b);
                st.finish_barriers_if_idle();
            }
            CatalogMsg::Confirm { item_id, reply } => {
                let _ = reply.send(st.confirm(&item_id));
            }
            CatalogMsg::Reject { item_id, correction, reply } => {
                let _ = reply.send(st.reject(&item_id, correction));
            }
            CatalogMsg::SetRole { item_id, role, similar, reply } => {
                let _ = reply.send(st.set_role(&item_id, role, similar));
            }
            CatalogMsg::Focus { item_id, reply } => {
                let _ = reply.send(st.focus(item_id));
            }
            CatalogMsg::SubmitBarcode { item_id, text, reply } => {
                let _ = reply.send(st.submit_barcode(&item_id, &text));
            }
            CatalogMsg::SkipBarcode { item_id, reply } => {
                let _ = reply.send(st.skip_barcode(&item_id));
            }
            CatalogMsg::SetMode { item_id, mode, reply } => {
                let _ = reply.send(st.set_mode(item_id, mode));
            }
            CatalogMsg::Items { filter, reply } => {
                let _ = reply.send(st.items(filter));
            }
            CatalogMsg::Item { item_id, reply } => {
                let _ = reply.send(st.recs.get(&item_id).map(|r| r.item.clone()));
            }
            CatalogMsg::AssignSkus { ids, reply } => {
                let _ = reply.send(st.assign_skus(ids));
            }
            CatalogMsg::Resume { reply } => {
                let _ = reply.send(st.resume());
            }
            CatalogMsg::Crash => panic!("catalog crash requested"),
        }
        Ok(())
    }
}
