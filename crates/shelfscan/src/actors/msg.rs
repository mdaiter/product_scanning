//! Messages between stages. One enum per stage.

use std::sync::Arc;

use image::RgbImage;
use ractor::RpcReplyPort;
use tokio::sync::oneshot;

use super::Credit;
use crate::error::Result;
use crate::traits::Storefront;
use crate::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierKind {
    /// Finish what is in flight and report.
    Flush,
    /// Flush, then start a new session (tracks and in-memory catalog are cleared).
    Reset,
}

/// Travels Detect → Track → Embed → Catalog behind every earlier frame.
pub struct Barrier {
    pub kind: BarrierKind,
    pub reply: oneshot::Sender<ScanSummary>,
}

pub enum DetectMsg {
    Frame { id: u64, frame: Frame, credit: Credit },
    Barrier(Barrier),
    /// Test hook: makes the actor panic so supervision can be exercised.
    #[doc(hidden)]
    Crash,
}

#[derive(Debug, Clone)]
pub struct TrackBox {
    pub track_id: u64,
    pub bbox: BBox,
    pub label: String,
    pub role: Role,
    pub promoted: bool,
}

pub enum TrackMsg {
    Detections { id: u64, ts_ms: u64, img: RgbImage, dets: Vec<Detection>, credit: Credit },
    Barrier(Barrier),
}

pub struct Candidate {
    pub track_id: u64,
    pub label: String,
    pub bbox: BBox,
    pub role: Role,
    pub crop: RgbImage,
    pub quality: f32,
    pub focus_bind: bool,
    pub ts_ms: u64,
}

pub enum EmbedMsg {
    Candidate(Candidate),
    TrackClosed(u64),
    Barrier(Barrier),
}

pub struct Observation {
    pub track_id: u64,
    pub embedding: Vec<f32>,
    pub jpeg: Vec<u8>,
    pub colors: Vec<String>,
    pub quality: f32,
    pub label: String,
    pub bbox: BBox,
    pub role: Role,
    pub focus_bind: bool,
    pub ts_ms: u64,
}

pub type Reply<T> = RpcReplyPort<Result<T>>;

pub enum CatalogMsg {
    // ── pipeline
    Observe(Observation),
    TrackClosed(u64),
    GuessDone { item_id: String, ticket: u64, epoch: u32, result: Result<ProductGuess> },
    LookupDone { item_id: String, gtin: String, epoch: u32, result: Result<Option<LookupResult>> },
    BarcodeReads { item_id: String, reads: Vec<BarcodeRead> },
    Barrier(Barrier),
    // ── commands
    Confirm { item_id: String, reply: Reply<CatalogItem> },
    Reject { item_id: String, correction: Option<String>, reply: Reply<CatalogItem> },
    SetRole { item_id: String, role: Role, similar: bool, reply: Reply<CatalogItem> },
    Focus { item_id: Option<String>, reply: Reply<()> },
    SubmitBarcode { item_id: String, text: String, reply: Reply<CatalogItem> },
    SkipBarcode { item_id: String, reply: Reply<CatalogItem> },
    SetMode { item_id: Option<String>, mode: ScanMode, reply: Reply<()> },
    Items { filter: ItemFilter, reply: RpcReplyPort<Vec<CatalogItem>> },
    Item { item_id: String, reply: RpcReplyPort<Option<CatalogItem>> },
    AssignSkus { ids: Vec<String>, reply: Reply<Vec<CatalogItem>> },
    Resume { reply: Reply<u32> },
    /// Test hook: makes the actor panic so supervision and restore can be exercised.
    #[doc(hidden)]
    Crash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemFilter {
    /// Everything, including hidden items.
    All,
    /// Role = Product.
    Visible,
    /// Products the user still has to look at.
    NeedsReview,
}

pub struct GuessJob {
    pub item_id: String,
    pub ticket: u64,
    pub epoch: u32,
    pub jpegs: Vec<Vec<u8>>,
    pub label: String,
    pub colors: Vec<String>,
    pub excluded: Vec<String>,
    pub mode: ScanMode,
}

pub enum GuessMsg {
    Guess(GuessJob),
}

pub enum LookupMsg {
    Lookup { item_id: String, gtin: String, epoch: u32 },
}

pub enum BarcodeMsg {
    Scan { item_id: String, frame: Frame, credit: Credit },
}

/// What the overlay needs to know about an item (pre-formatted by Catalog).
#[derive(Debug, Clone)]
pub struct OverlayItem {
    pub item_id: String,
    pub state: ItemState,
    pub role: Role,
    pub label: String,
    pub needs_barcode: bool,
}

pub enum OverlayMsg {
    Boxes { id: u64, ts_ms: u64, boxes: Vec<TrackBox> },
    Bind { track_id: u64, item_id: String },
    Item(OverlayItem),
    Rebind { from: String, into: String },
    Focus(Option<String>),
    Reset,
}

// Messages are moved once into a mailbox; boxing the rare large variant would only add noise.
#[allow(clippy::large_enum_variant)]
pub enum ListsMsg {
    Generate { include_unconfirmed: bool, reply: Reply<Vec<Listing>> },
    Update { listing: Listing, reply: Reply<Listing> },
    Get { ids: Option<Vec<String>>, reply: RpcReplyPort<Vec<Listing>> },
    MarkPublished { receipts: Vec<PublishReceipt> },
    Reset,
}

pub enum PublishMsg {
    Publish { ids: Vec<String>, storefront: Arc<dyn Storefront>, reply: Reply<Vec<PublishReceipt>> },
}

pub enum PricingMsg {
    Suggest { item_id: String, refresh: bool, reply: Reply<PriceSuggestion> },
}

pub enum EventMsg {
    Emit(Event),
}
