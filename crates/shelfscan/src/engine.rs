//! The public face of the library: build an [`Engine`], push frames, listen to events, send commands.
//!
//! Commands that wait for an answer are `async` (Swift `async`, Kotlin `suspend`, Python `asyncio`).
//! Their futures only await oneshot channels, so any executor can drive them. Fire-and-forget calls
//! (`push_frame`, `stats`, ...) are plain functions that never block.

use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant},
};

use ractor::{Actor, ActorRef};
use tokio::{runtime::Runtime, sync::oneshot};

use crate::actors::{msg::*, supervisor::*, Credits, Plugins, Shared, Slots};
use crate::backends::{BlobDetector, ColorEmbedder, PlaceholderGuesser, StubPriceAdvisor, TemplateWriter};
use crate::error::{Result, ShelfError};
use crate::store::{MemoryStore, Store};
use crate::traits::*;
use crate::types::*;

/// Minimum spacing between frames sent to the barcode scanner while an item is focused.
const BARCODE_MIN_INTERVAL_MS: u64 = 120;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(120);

struct BuilderInner {
    cfg: EngineConfig,
    detector: Option<Arc<dyn Detector>>,
    embedder: Option<Arc<dyn Embedder>>,
    scanner: Option<Arc<dyn BarcodeScanner>>,
    guesser: Option<Arc<dyn ProductGuesser>>,
    lookup: Option<Arc<dyn ProductLookup>>,
    writer: Option<Arc<dyn ListingWriter>>,
    advisor: Option<Arc<dyn PriceAdvisor>>,
    store: Option<Arc<dyn Store>>,
    listeners: Vec<Arc<dyn EventListener>>,
}

/// Collects configuration and plugins. Anything you do not set uses the built-in demo plugin.
#[derive(uniffi::Object)]
pub struct EngineBuilder {
    inner: Mutex<BuilderInner>,
}

impl EngineBuilder {
    fn with<F: FnOnce(&mut BuilderInner)>(self: Arc<Self>, f: F) -> Arc<Self> {
        if let Ok(mut g) = self.inner.lock() {
            f(&mut g);
        }
        self
    }

    /// Use your own [`Store`] (Rust only; foreign code uses `sqlite_store`).
    pub fn store(self: Arc<Self>, store: Arc<dyn Store>) -> Arc<Self> {
        self.with(|b| b.store = Some(store))
    }
}

#[uniffi::export]
impl EngineBuilder {
    #[uniffi::constructor]
    pub fn new(config: EngineConfig) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(BuilderInner {
                cfg: config,
                detector: None,
                embedder: None,
                scanner: None,
                guesser: None,
                lookup: None,
                writer: None,
                advisor: None,
                store: None,
                listeners: vec![],
            }),
        })
    }

    pub fn detector(self: Arc<Self>, d: Arc<dyn Detector>) -> Arc<Self> {
        self.with(|b| b.detector = Some(d))
    }
    pub fn embedder(self: Arc<Self>, e: Arc<dyn Embedder>) -> Arc<Self> {
        self.with(|b| b.embedder = Some(e))
    }
    pub fn barcode_scanner(self: Arc<Self>, s: Arc<dyn BarcodeScanner>) -> Arc<Self> {
        self.with(|b| b.scanner = Some(s))
    }
    pub fn guesser(self: Arc<Self>, g: Arc<dyn ProductGuesser>) -> Arc<Self> {
        self.with(|b| b.guesser = Some(g))
    }
    pub fn product_lookup(self: Arc<Self>, l: Arc<dyn ProductLookup>) -> Arc<Self> {
        self.with(|b| b.lookup = Some(l))
    }
    pub fn listing_writer(self: Arc<Self>, w: Arc<dyn ListingWriter>) -> Arc<Self> {
        self.with(|b| b.writer = Some(w))
    }
    pub fn price_advisor(self: Arc<Self>, a: Arc<dyn PriceAdvisor>) -> Arc<Self> {
        self.with(|b| b.advisor = Some(a))
    }
    pub fn listener(self: Arc<Self>, l: Arc<dyn EventListener>) -> Arc<Self> {
        self.with(|b| b.listeners.push(l))
    }

    /// Identify products and write listing copy with the Anthropic API (`model = None` uses `claude-sonnet-5-5`).
    /// Only masked product crops are sent. On a phone, point your own backend at the API instead of shipping a key.
    #[cfg(feature = "net")]
    pub fn anthropic(self: Arc<Self>, api_key: String, model: Option<String>) -> Arc<Self> {
        let g = crate::cloud::anthropic::AnthropicGuesser::new(api_key.clone(), model.clone());
        let w = crate::cloud::anthropic::AnthropicWriter::new(api_key, model);
        self.with(|b| {
            b.guesser = Some(g);
            b.writer = Some(w);
        })
    }

    /// Look barcodes up in Open Food Facts. `contact` (an email or URL) goes into the User-Agent as their API asks.
    #[cfg(feature = "net")]
    pub fn open_food_facts(self: Arc<Self>, contact: String) -> Arc<Self> {
        let l = crate::cloud::off::OpenFoodFactsLookup::new(contact);
        self.with(|b| b.lookup = Some(l))
    }

    /// Keep scans, the SKU registry, the fixture library and the price cache in a SQLite file.
    #[cfg(feature = "sqlite")]
    pub fn sqlite_store(self: Arc<Self>, path: String) -> std::result::Result<Arc<Self>, ShelfError> {
        let s = crate::store::SqliteStore::open(path)?;
        Ok(self.with(|b| b.store = Some(Arc::new(s))))
    }

    pub fn build(&self) -> std::result::Result<Arc<Engine>, ShelfError> {
        let mut b = self.inner.lock().map_err(|_| ShelfError::internal("builder poisoned"))?;
        let mut cfg = b.cfg.clone();
        cfg.frame_credits = cfg.frame_credits.max(1);
        cfg.worker_threads = cfg.worker_threads.max(1);
        cfg.blocking_threads = cfg.blocking_threads.max(2);
        cfg.max_images_per_item = cfg.max_images_per_item.max(1);
        cfg.barcode_votes = cfg.barcode_votes.max(1);
        if cfg.analysis_fps.is_nan() || cfg.analysis_fps <= 0.0 {
            return Err(ShelfError::invalid("analysis_fps must be positive"));
        }

        let plugins = Plugins {
            detector: b.detector.take().unwrap_or_else(|| Arc::new(BlobDetector::default())),
            embedder: b.embedder.take().unwrap_or_else(|| Arc::new(ColorEmbedder::default())),
            scanner: b.scanner.take().or_else(default_scanner),
            guesser: b.guesser.take().unwrap_or_else(|| Arc::new(PlaceholderGuesser)),
            lookup: b.lookup.take(),
            writer: b.writer.take().unwrap_or_else(|| Arc::new(TemplateWriter)),
            advisor: b.advisor.take().unwrap_or_else(|| Arc::new(StubPriceAdvisor)),
        };
        let store: Arc<dyn Store> = b.store.take().unwrap_or_else(|| Arc::new(MemoryStore::new()));
        let listeners = std::mem::take(&mut b.listeners);
        drop(b);

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(cfg.worker_threads as usize)
            .max_blocking_threads(cfg.blocking_threads as usize)
            .thread_name("shelfscan")
            .enable_all()
            .build()
            .map_err(|e| ShelfError::internal(format!("cannot start runtime: {e}")))?;

        let shared = Arc::new(Shared {
            stats: Stats::default(),
            store,
            plugins,
            slots: Slots::default(),
            focus: RwLock::new(None),
            listeners: RwLock::new(listeners),
            frame_credits: Credits::new(cfg.frame_credits as usize),
            barcode_credits: Credits::new(1),
            lossy_inflight: AtomicUsize::new(0),
            started: Instant::now(),
            frame_seq: AtomicU64::new(0),
            last_admit_ms: AtomicU64::new(0),
            last_barcode_ms: AtomicU64::new(0),
            cfg,
        });
        // Start the pipeline on the engine's own runtime and wait on a plain channel. `block_on`
        // would panic if `build()` is called from inside another async runtime.
        let sh = shared.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        rt.spawn(async move {
            let _ = tx.send(Actor::spawn(None, SupervisorActor, sh).await);
        });
        let (sup, _handle) = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| ShelfError::internal("pipeline did not start in time"))?
            .map_err(|e| ShelfError::internal(format!("cannot start pipeline: {e}")))?;
        Ok(Arc::new(Engine { shared, handle: rt.handle().clone(), rt: Mutex::new(Some(rt)), supervisor: Mutex::new(Some(sup)), closed: AtomicBool::new(false) }))
    }
}

fn default_scanner() -> Option<Arc<dyn BarcodeScanner>> {
    #[cfg(feature = "barcode")]
    {
        Some(Arc::new(crate::backends::RxingScanner))
    }
    #[cfg(not(feature = "barcode"))]
    {
        None
    }
}

#[derive(uniffi::Object)]
pub struct Engine {
    shared: Arc<Shared>,
    handle: tokio::runtime::Handle,
    rt: Mutex<Option<Runtime>>,
    supervisor: Mutex<Option<ActorRef<SupMsg>>>,
    closed: AtomicBool,
}

impl Engine {
    /// Run `fut` on the engine's runtime and hand the answer back through a oneshot channel, so the
    /// future we return is runtime-agnostic.
    async fn run<T: Send + 'static>(&self, fut: impl Future<Output = Result<T>> + Send + 'static) -> Result<T> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ShelfError::Closed);
        }
        let (tx, rx) = oneshot::channel();
        self.handle.spawn(async move {
            let _ = tx.send(fut.await);
        });
        rx.await.map_err(|_| ShelfError::Closed)?
    }

    async fn barrier(&self, kind: BarrierKind) -> Result<ScanSummary> {
        let sh = self.shared.clone();
        self.run(async move {
            let (tx, rx) = oneshot::channel();
            if !sh.slots.detect.cast(DetectMsg::Barrier(Barrier { kind, reply: tx })) {
                return Err(ShelfError::Closed);
            }
            match tokio::time::timeout(FLUSH_TIMEOUT, rx).await {
                Ok(Ok(s)) => Ok(s),
                Ok(Err(_)) => Err(ShelfError::internal("a pipeline stage restarted before the flush finished; try again")),
                Err(_) => Err(ShelfError::backend("flush", "timed out waiting for guesses or lookups to finish")),
            }
        })
        .await
    }

    #[doc(hidden)]
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Rust-only: the runtime handle, e.g. to `block_on` the async commands from a CLI.
    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.handle.clone()
    }
}

#[uniffi::export]
impl Engine {
    // ───────────── frames ─────────────

    /// Cheap pre-check: would a frame with this timestamp be admitted right now? Lets the camera
    /// callback skip building a `Frame` at all. `0` means "now".
    pub fn wants_frame(&self, timestamp_ms: u64) -> bool {
        let sh = &self.shared;
        let ts = if timestamp_ms == 0 { sh.now_ms() } else { timestamp_ms };
        self.rate_ok(ts) && sh.frame_credits.available() > 0
    }

    /// Offer a camera frame. Returns `true` if it entered the pipeline. Frames that arrive too fast
    /// or while the pipeline is full are dropped here, for free, on the caller's thread.
    pub fn push_frame(&self, frame: Frame) -> bool {
        let sh = &self.shared;
        let mut frame = frame;
        sh.stats.frames_in.fetch_add(1, Ordering::Relaxed);
        if frame.timestamp_ms == 0 {
            frame.timestamp_ms = sh.now_ms();
        }
        let ts = frame.timestamp_ms;

        // Barcode path: only while an item is focused, at its own pace, on the full-resolution frame.
        if sh.plugins.scanner.is_some() {
            if let Some(item_id) = sh.focus() {
                let last = sh.last_barcode_ms.load(Ordering::Relaxed);
                if ts < last || ts - last >= BARCODE_MIN_INTERVAL_MS {
                    if let Some(credit) = sh.barcode_credits.try_acquire() {
                        sh.last_barcode_ms.store(ts, Ordering::Relaxed);
                        sh.slots.barcode.cast(BarcodeMsg::Scan { item_id, frame: frame.clone(), credit });
                    }
                }
            }
        }

        if !self.rate_ok(ts) {
            sh.stats.frames_skipped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let Some(credit) = sh.frame_credits.try_acquire() else {
            sh.stats.frames_dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        sh.last_admit_ms.store(ts, Ordering::Relaxed);
        let id = sh.frame_seq.fetch_add(1, Ordering::Relaxed) + 1;
        if !sh.slots.detect.cast(DetectMsg::Frame { id, frame, credit }) {
            sh.stats.frames_dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    pub fn stats(&self) -> ScanSummary {
        let mut s = self.shared.stats.snapshot();
        s.frames_dropped = s.frames_dropped.min(s.frames_in);
        s
    }

    /// The item currently being scanned from several angles, if any.
    pub fn focused_item(&self) -> Option<String> {
        self.shared.focus()
    }

    pub fn add_listener(&self, listener: Arc<dyn EventListener>) {
        if let Ok(mut g) = self.shared.listeners.write() {
            g.push(listener);
        }
    }

    // ───────────── scan control ─────────────

    /// Finish everything in flight (frames, guesses, lookups) and report. Call when the user stops scanning.
    pub async fn flush(&self) -> std::result::Result<ScanSummary, ShelfError> {
        self.barrier(BarrierKind::Flush).await
    }

    /// Flush, then start a new session. Saved items stay in the store; call `resume` to load them again.
    pub async fn new_session(&self) -> std::result::Result<ScanSummary, ShelfError> {
        self.barrier(BarrierKind::Reset).await
    }

    /// Load items saved by earlier sessions into this one. Returns how many were added.
    pub async fn resume(&self) -> std::result::Result<u32, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::Resume { reply }).await? }).await
    }

    /// Delete every saved item from the store (the SKU registry and fixture library are kept).
    pub async fn clear_saved(&self) -> std::result::Result<(), ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.store.clear_items() }).await
    }

    // ───────────── catalog ─────────────

    pub async fn catalog(&self, include_hidden: bool) -> std::result::Result<Vec<CatalogItem>, ShelfError> {
        let sh = self.shared.clone();
        let filter = if include_hidden { ItemFilter::All } else { ItemFilter::Visible };
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::Items { filter, reply }).await }).await
    }

    /// Products the user still has to confirm or correct.
    pub async fn review_queue(&self) -> std::result::Result<Vec<CatalogItem>, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::Items { filter: ItemFilter::NeedsReview, reply }).await }).await
    }

    pub async fn item(&self, item_id: String) -> std::result::Result<Option<CatalogItem>, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::Item { item_id, reply }).await }).await
    }

    /// ✓ The guess is right.
    pub async fn confirm_item(&self, item_id: String) -> std::result::Result<CatalogItem, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::Confirm { item_id, reply }).await? }).await
    }

    /// ✗ The guess is wrong. With `correction` the typed name becomes the user-confirmed title;
    /// without it the guesser is asked again (up to `guess_retries` times) and never repeats a rejected title.
    pub async fn reject_item(&self, item_id: String, correction: Option<String>) -> std::result::Result<CatalogItem, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::Reject { item_id, correction, reply }).await? }).await
    }

    /// Mark something as product / background / excluded. Hiding is reversible and never deletes.
    /// `similar` also hides every item that looks the same. Excluding teaches the fixture library.
    pub async fn set_role(&self, item_id: String, role: Role, similar: bool) -> std::result::Result<CatalogItem, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::SetRole { item_id, role, similar, reply }).await? }).await
    }

    /// Start a capture session for one item: its next views and barcode reads go to it.
    pub async fn focus_item(&self, item_id: String) -> std::result::Result<(), ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::Focus { item_id: Some(item_id), reply }).await? }).await
    }

    pub async fn clear_focus(&self) -> std::result::Result<(), ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::Focus { item_id: None, reply }).await? }).await
    }

    /// The user typed or scanned a barcode by hand. Validated (length and check digit) before use.
    pub async fn submit_barcode(&self, item_id: String, text: String) -> std::result::Result<CatalogItem, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::SubmitBarcode { item_id, text, reply }).await? }).await
    }

    /// "No barcode on this one": stop asking.
    pub async fn skip_barcode(&self, item_id: String) -> std::result::Result<CatalogItem, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::SkipBarcode { item_id, reply }).await? }).await
    }

    /// Session default (`item_id = None`) or a per-item override.
    pub async fn set_mode(&self, item_id: Option<String>, mode: ScanMode) -> std::result::Result<(), ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.catalog.ask(|reply| CatalogMsg::SetMode { item_id, mode, reply }).await? }).await
    }

    // ───────────── listings ─────────────

    /// Draft one listing per visible product. Confirmed items always; guessed ones only if asked.
    pub async fn generate_listings(&self, include_unconfirmed: bool) -> std::result::Result<Vec<Listing>, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.lists.ask(|reply| ListsMsg::Generate { include_unconfirmed, reply }).await? }).await
    }

    pub async fn update_listing(&self, listing: Listing) -> std::result::Result<Listing, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.lists.ask(|reply| ListsMsg::Update { listing, reply }).await? }).await
    }

    pub async fn listings(&self) -> std::result::Result<Vec<Listing>, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.lists.ask(|reply| ListsMsg::Get { ids: None, reply }).await }).await
    }

    /// Send listings (always as drafts) to a storefront.
    pub async fn publish(&self, listing_ids: Vec<String>, storefront: Arc<dyn Storefront>) -> std::result::Result<Vec<PublishReceipt>, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.publish.ask(|reply| PublishMsg::Publish { ids: listing_ids, storefront, reply }).await? }).await
    }

    /// Comparable prices for one item. Only runs when you call it.
    pub async fn suggest_price(&self, item_id: String, refresh: bool) -> std::result::Result<PriceSuggestion, ShelfError> {
        let sh = self.shared.clone();
        self.run(async move { sh.slots.pricing.ask(|reply| PricingMsg::Suggest { item_id, refresh, reply }).await? }).await
    }

    // ───────────── lifecycle ─────────────

    /// Stop all stages and the runtime. Further calls fail with `Closed`. Also happens on drop.
    pub fn shutdown(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(sup) = self.supervisor.lock().ok().and_then(|mut g| g.take()) {
            sup.stop(Some("engine shutdown".into()));
        }
        if let Some(rt) = self.rt.lock().ok().and_then(|mut g| g.take()) {
            rt.shutdown_background(); // never block (or panic) when called from async code
        }
    }
}

impl Engine {
    /// Test hook: make one stage panic ("detect" or "catalog") to exercise supervision.
    #[doc(hidden)]
    pub fn crash_stage_for_tests(&self, stage: &str) {
        match stage {
            "detect" => {
                self.shared.slots.detect.cast(DetectMsg::Crash);
            }
            "catalog" => {
                self.shared.slots.catalog.cast(CatalogMsg::Crash);
            }
            other => panic!("no crash hook for stage {other}"),
        }
    }

    fn rate_ok(&self, ts: u64) -> bool {
        let interval = (1000.0 / self.shared.cfg.analysis_fps.max(0.1)) as u64;
        let last = self.shared.last_admit_ms.load(Ordering::Relaxed);
        last == 0 || ts < last || ts - last >= interval
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        if let Some(sup) = self.supervisor.get_mut().ok().and_then(|g| g.take()) {
            sup.stop(None);
        }
        if let Some(rt) = self.rt.get_mut().ok().and_then(|g| g.take()) {
            rt.shutdown_background(); // never block (or panic) when dropped from an async context
        }
    }
}
