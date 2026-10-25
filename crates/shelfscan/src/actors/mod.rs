//! The pipeline: one ractor actor per stage, wired through [`Shared`].
//!
//! ```text
//! push_frame ─▶ Detect ─▶ Track ─▶ Embed ─▶ Catalog ◀─▶ Guess / Lookup
//!   │ (credit,     │        └──────────▶ Overlay ◀────────┘     │
//!   │  fps gate)   │                        │                  ▼
//!   └─▶ Barcode ───┴────────────────────────┴──▶ Events ─▶ foreign listeners
//!                  Lists ─▶ Publish      Pricing        Supervisor (restarts the rest)
//! ```
//!
//! Rules that keep this predictable:
//! * ractor mailboxes are unbounded, so backpressure is explicit: a frame needs a [`Credit`] to
//!   enter and returns it when it leaves the frame path (or is dropped anywhere).
//! * Only `Catalog` mutates item state. Everyone else sends it messages.
//! * Plugins are synchronous and run through [`blocking`], so a slow model never stalls scheduling.
//! * Flush/reset travel through the mailboxes as a [`msg::Barrier`], which is FIFO-ordered behind
//!   every frame sent before it; `Catalog` completes it once pending guesses and lookups finish.

use std::{
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, RwLock,
    },
    time::Instant,
};

use ractor::ActorRef;

use crate::error::{Result, ShelfError};
use crate::store::Store;
use crate::traits::*;
use crate::types::*;

pub mod barcode;
pub mod catalog;
pub mod detect;
pub mod embed;
pub mod events;
pub mod guess;
pub mod lists;
pub mod msg;
pub mod overlay;
pub mod pricing;
pub mod publish;
pub mod supervisor;
pub mod track;

use msg::*;

/// Return type of ractor's `Actor` methods (the plain `Result` in this crate is `ShelfError`).
pub type ActorResult<T> = std::result::Result<T, ractor::ActorProcessingErr>;

// ───────────────────────────── backpressure ─────────────────────────────

/// A counting semaphore without waiting: `try_acquire` either succeeds now or the frame is dropped.
pub struct Credits(AtomicUsize);

/// RAII permit. Dropping it (anywhere, including when a message is discarded) returns the credit.
pub struct Credit(Arc<Credits>);

impl Credits {
    pub fn new(n: usize) -> Arc<Self> {
        Arc::new(Self(AtomicUsize::new(n)))
    }
    pub fn try_acquire(self: &Arc<Self>) -> Option<Credit> {
        let mut cur = self.0.load(Ordering::Acquire);
        loop {
            if cur == 0 {
                return None;
            }
            match self.0.compare_exchange_weak(cur, cur - 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Credit(self.clone())),
                Err(v) => cur = v,
            }
        }
    }
    pub fn available(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

impl Drop for Credit {
    fn drop(&mut self) {
        self.0 .0.fetch_add(1, Ordering::AcqRel);
    }
}

// ───────────────────────────── shared state ─────────────────────────────

/// The current ActorRef for one stage. Replaced by the supervisor when a stage is restarted.
pub struct Slot<M: ractor::Message>(RwLock<Option<ActorRef<M>>>);

impl<M: ractor::Message> Default for Slot<M> {
    fn default() -> Self {
        Self(RwLock::new(None))
    }
}

impl<M: ractor::Message> Slot<M> {
    pub fn set(&self, r: ActorRef<M>) {
        if let Ok(mut g) = self.0.write() {
            *g = Some(r);
        }
    }
    pub fn get(&self) -> Option<ActorRef<M>> {
        self.0.read().ok().and_then(|g| g.clone())
    }
    /// Fire and forget. `false` when the stage is not running (the message is dropped).
    pub fn cast(&self, m: M) -> bool {
        self.get().map(|r| r.cast(m).is_ok()).unwrap_or(false)
    }
    /// Request/response. Errors map to `Closed` (stage gone) or `Internal` (no reply).
    pub async fn ask<T: Send + 'static>(&self, f: impl FnOnce(ractor::RpcReplyPort<T>) -> M) -> Result<T> {
        let r = self.get().ok_or(ShelfError::Closed)?;
        match r.call(f, None).await {
            Ok(ractor::rpc::CallResult::Success(v)) => Ok(v),
            Ok(_) => Err(ShelfError::internal("stage did not answer (it may have restarted)")),
            Err(_) => Err(ShelfError::Closed),
        }
    }
}

#[derive(Default)]
pub struct Slots {
    pub detect: Slot<DetectMsg>,
    pub track: Slot<TrackMsg>,
    pub embed: Slot<EmbedMsg>,
    pub catalog: Slot<CatalogMsg>,
    pub guess: Slot<GuessMsg>,
    pub barcode: Slot<BarcodeMsg>,
    pub lookup: Slot<LookupMsg>,
    pub overlay: Slot<OverlayMsg>,
    pub lists: Slot<ListsMsg>,
    pub publish: Slot<PublishMsg>,
    pub pricing: Slot<PricingMsg>,
    pub events: Slot<EventMsg>,
}

pub struct Plugins {
    pub detector: Arc<dyn Detector>,
    pub embedder: Arc<dyn Embedder>,
    pub scanner: Option<Arc<dyn BarcodeScanner>>,
    pub guesser: Arc<dyn ProductGuesser>,
    pub lookup: Option<Arc<dyn ProductLookup>>,
    pub writer: Arc<dyn ListingWriter>,
    pub advisor: Arc<dyn PriceAdvisor>,
}

/// Overlay/Stats events are lossy: at most this many may be queued for delivery.
const LOSSY_MAX: usize = 2;

pub struct Shared {
    pub cfg: EngineConfig,
    pub stats: Stats,
    pub store: Arc<dyn Store>,
    pub plugins: Plugins,
    pub slots: Slots,
    /// The item currently being scanned from several angles. Written by Catalog only.
    pub focus: RwLock<Option<String>>,
    pub listeners: RwLock<Vec<Arc<dyn EventListener>>>,
    pub frame_credits: Arc<Credits>,
    pub barcode_credits: Arc<Credits>,
    pub lossy_inflight: AtomicUsize,
    pub started: Instant,
    pub frame_seq: AtomicU64,
    pub last_admit_ms: AtomicU64,
    pub last_barcode_ms: AtomicU64,
}

impl Shared {
    pub fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64 + 1
    }

    pub fn focus(&self) -> Option<String> {
        self.focus.read().ok().and_then(|g| g.clone())
    }

    pub fn set_focus(&self, f: Option<String>) {
        if let Ok(mut g) = self.focus.write() {
            *g = f;
        }
    }

    pub fn emit(&self, e: Event) {
        let lossy = events::is_lossy(&e);
        if lossy && self.lossy_inflight.fetch_add(1, Ordering::AcqRel) >= LOSSY_MAX {
            self.lossy_inflight.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        if !self.slots.events.cast(EventMsg::Emit(e)) && lossy {
            self.lossy_inflight.fetch_sub(1, Ordering::AcqRel);
        }
    }

    pub fn warn(&self, source: &str, message: impl ToString) {
        self.emit(Event::Warning { source: source.into(), message: message.to_string() });
    }

    pub fn error(&self, source: &str, message: impl ToString) {
        self.emit(Event::Error { source: source.into(), message: message.to_string() });
    }
}

/// Run a synchronous plugin call on the blocking pool. A panic inside the plugin becomes an error
/// instead of killing the calling actor.
pub async fn blocking<T: Send + 'static>(name: &'static str, f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) => Err(ShelfError::backend(name, format!("plugin panicked or was cancelled: {e}"))),
    }
}
