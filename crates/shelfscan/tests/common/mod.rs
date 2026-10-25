#![allow(dead_code)]
//! Test doubles and helpers shared by the integration tests.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use shelfscan::*;

pub fn config() -> EngineConfig {
    EngineConfig { analysis_fps: 20.0, novelty_threshold: 0.0, blur_gate_ratio: 0.0, ..EngineConfig::default() }
}

pub fn engine_with(cfg: EngineConfig, f: impl FnOnce(Arc<EngineBuilder>) -> Arc<EngineBuilder>) -> (Arc<Engine>, Arc<EventLog>) {
    let log = EventLog::new();
    let b = EngineBuilder::new(cfg).listener(log.clone());
    (f(b).build().expect("engine builds"), log)
}

pub fn engine() -> (Arc<Engine>, Arc<EventLog>) {
    engine_with(config(), |b| b)
}

/// Push frames at the pace the pipeline accepts them (waits for a free credit instead of dropping).
pub fn pump(e: &Engine, frames: Vec<Frame>) {
    for f in frames {
        let mut tries = 0;
        while !e.wants_frame(f.timestamp_ms) {
            std::thread::sleep(Duration::from_millis(2));
            tries += 1;
            assert!(tries < 5000, "pipeline never freed a credit");
        }
        assert!(e.push_frame(f), "frame should be accepted");
    }
}

pub fn scan(e: &Engine, scene: &synthetic::Scene, n: usize) {
    pump(e, scene.frames(n, 6.0, 1000, 11));
}

pub fn wait(log: &EventLog, pred: impl Fn(&Event) -> bool) -> Event {
    log.wait_for(pred, Duration::from_secs(10)).expect("event arrived in time")
}

pub fn title_of(items: &[CatalogItem], color: &str) -> Option<CatalogItem> {
    items.iter().find(|i| i.guess.title.to_lowercase().contains(color)).cloned()
}

// ───────────── doubles ─────────────

/// Returns whatever it is told, for any frame.
pub struct FakeScanner(pub Mutex<Vec<BarcodeRead>>);
impl BarcodeScanner for FakeScanner {
    fn scan(&self, _f: Frame) -> Result<Vec<BarcodeRead>, ShelfError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

pub fn read(text: &str) -> BarcodeRead {
    BarcodeRead { text: text.into(), symbology: "EAN_13".into() }
}

pub struct FakeLookup;
impl ProductLookup for FakeLookup {
    fn lookup(&self, gtin: String) -> Result<Option<LookupResult>, ShelfError> {
        Ok((gtin == "4006381333931").then(|| LookupResult { title: Some("Stabilo Boss Highlighter".into()), brand: Some("Stabilo".into()), category: Some("Office".into()), source: "fixture".into() }))
    }
}

/// Slow guesser that counts calls and can panic on the first one.
pub struct SlowGuesser {
    pub delay: Duration,
    pub calls: AtomicUsize,
    pub panic_first: bool,
}
impl ProductGuesser for SlowGuesser {
    fn guess(&self, req: GuessRequest) -> Result<ProductGuess, ShelfError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.panic_first && n == 0 {
            panic!("guesser exploded");
        }
        std::thread::sleep(self.delay);
        let color = req.dominant_colors.first().cloned().unwrap_or_default();
        Ok(ProductGuess { title: format!("Guess {n} {color}"), brand: None, category: None, attributes: vec![], confidence: 0.5 })
    }
}

pub struct SlowDetector(pub Duration, pub Arc<dyn Detector>);
impl Detector for SlowDetector {
    fn detect(&self, f: Frame) -> Result<Vec<Detection>, ShelfError> {
        std::thread::sleep(self.0);
        self.1.detect(f)
    }
}

/// Records what it was asked to publish.
#[derive(Default)]
pub struct RecordingStorefront {
    pub seen: Mutex<Vec<Listing>>,
    pub fail_ids: Mutex<Vec<String>>,
}
impl Storefront for RecordingStorefront {
    fn name(&self) -> String {
        "recording".into()
    }
    fn publish(&self, listings: Vec<Listing>) -> Result<Vec<PublishReceipt>, ShelfError> {
        let fails = self.fail_ids.lock().unwrap().clone();
        let mut out = vec![];
        for l in listings {
            let ok = !fails.contains(&l.id);
            out.push(PublishReceipt { listing_id: l.id.clone(), ok, remote_id: ok.then(|| format!("remote-{}", l.id)), url: None, message: if ok { "ok".into() } else { "rejected".into() } });
            self.seen.lock().unwrap().push(l);
        }
        Ok(out)
    }
}

/// A writer that misbehaves: marks the listing Active and skips the price.
pub struct PushyWriter;
impl ListingWriter for PushyWriter {
    fn write(&self, item: CatalogItem, _style: String) -> Result<Listing, ShelfError> {
        Ok(Listing {
            id: "whatever".into(),
            source_item_id: "wrong".into(),
            title: item.guess.title,
            description_html: "<p>x</p>".into(),
            vendor: String::new(),
            product_type: String::new(),
            tags: vec![],
            status: ListingStatus::Active,
            options: vec![],
            variants: vec![],
            images: vec![],
            seo_title: None,
            seo_description: None,
            confidence: 0.0,
            published: true,
        })
    }
}

/// Poll an item until `pred` holds (guesses and lookups complete asynchronously).
pub async fn wait_item(e: &Engine, id: &str, pred: impl Fn(&CatalogItem) -> bool) -> CatalogItem {
    for _ in 0..2000 {
        if let Some(i) = e.item(id.to_string()).await.unwrap() {
            if pred(&i) {
                return i;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("item {id} never reached the expected state: {:?}", e.item(id.to_string()).await.unwrap());
}

pub async fn wait_guessed(e: &Engine, n: usize) -> Vec<CatalogItem> {
    for _ in 0..2000 {
        let items = e.catalog(false).await.unwrap();
        if items.len() >= n && items.iter().all(|i| i.guess_ready) {
            return items;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("never saw {n} guessed items: {:?}", e.catalog(true).await.unwrap().iter().map(|i| (&i.id, i.state)).collect::<Vec<_>>());
}
