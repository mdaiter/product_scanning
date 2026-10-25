//! Plain data types. Everything here crosses the FFI boundary by value.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

// ───────────────────────────── pixels ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PixelFormat {
    /// 3 bytes/px, R G B.
    Rgb8,
    /// 4 bytes/px, R G B A (Android `RGBA_8888`, most image libs).
    Rgba8,
    /// 4 bytes/px, B G R A (iOS `kCVPixelFormatType_32BGRA`, the cheapest iOS camera format).
    Bgra8,
    /// 1 byte/px luma.
    Gray8,
    /// Y plane (`stride * height` bytes) followed by an interleaved UV plane
    /// (`stride * height / 2` bytes). iOS `420YpCbCr8BiPlanar`.
    Nv12,
}

/// A camera frame. Plugins always receive `Rgb8`, tightly packed (`stride == 0`).
#[derive(Clone, uniffi::Record)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
    /// Bytes per row of plane 0. `0` means tightly packed.
    #[uniffi(default = 0)]
    pub stride: u32,
    /// Capture time in milliseconds on any monotonic clock. `0` = let the engine stamp it.
    #[uniffi(default = 0)]
    pub timestamp_ms: u64,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{} {:?}, {} bytes, t={}ms)", self.width, self.height, self.format, self.data.len(), self.timestamp_ms)
    }
}

// ───────────────────────────── geometry & detection ─────────────────────────────

/// Axis-aligned box in **normalised** coordinates (0..1, origin top-left).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct BBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BBox {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
    pub fn clamp01(&self) -> BBox {
        let x0 = self.x.clamp(0.0, 1.0);
        let y0 = self.y.clamp(0.0, 1.0);
        let x1 = (self.x + self.w).clamp(0.0, 1.0);
        let y1 = (self.y + self.h).clamp(0.0, 1.0);
        BBox { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
    }
    pub fn iou(&self, o: &BBox) -> f32 {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
        let union = self.area() + o.area() - inter;
        if union <= f32::EPSILON { 0.0 } else { inter / union }
    }
    /// True when the box touches the frame edge (object probably truncated).
    pub fn touches_border(&self, eps: f32) -> bool {
        self.x <= eps || self.y <= eps || self.x + self.w >= 1.0 - eps || self.y + self.h >= 1.0 - eps
    }
    /// Pixel rect `(x, y, w, h)` for an image of the given size; always at least 1x1 and inside.
    pub fn to_pixels(&self, img_w: u32, img_h: u32) -> (u32, u32, u32, u32) {
        let b = self.clamp01();
        let x = ((b.x * img_w as f32).floor() as u32).min(img_w.saturating_sub(1));
        let y = ((b.y * img_h as f32).floor() as u32).min(img_h.saturating_sub(1));
        let w = ((b.w * img_w as f32).ceil() as u32).clamp(1, img_w - x);
        let h = ((b.h * img_h as f32).ceil() as u32).clamp(1, img_h - y);
        (x, y, w, h)
    }
}

/// Instance mask covering the detection's box (0 = background, 255 = object).
/// `data.len() == width * height`; the mask is stretched over the box.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Detection {
    pub bbox: BBox,
    pub score: f32,
    pub label: String,
    #[uniffi(default = None)]
    pub mask: Option<Mask>,
}

/// One barcode decoded from a frame.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BarcodeRead {
    pub text: String,
    /// "EAN_13", "UPC_A", "QR_CODE", ... (whatever the scanner reports).
    pub symbology: String,
}

// ───────────────────────────── identity ─────────────────────────────

/// How an item is identified. A session default, overridable per item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum ScanMode {
    /// Catalog mode: capture several views, flip for the barcode, treat the GTIN as ground truth.
    BarcodeFirst,
    /// Handmade / unbranded: describe from images, the user approves the text.
    DescribeFirst,
}

/// Whether an item is something the user wants to sell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum Role {
    Product,
    /// Probably decor/fixtures/shelves: hidden by default, never deleted.
    Background,
    /// Hidden because the user said so (or it matched the fixture library).
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum ConfirmedBy {
    User,
    Barcode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum ItemState {
    Unknown,
    Guessed,
    Confirmed { by: ConfirmedBy },
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct GuessRequest {
    pub item_id: String,
    /// Best-quality **masked crop** of the product (RGB8). Never a full camera frame.
    pub image: Frame,
    /// Further views of the same product, best first (also masked crops).
    pub extra_views: Vec<Frame>,
    /// Label from the detector ("bottle", "product", ...).
    pub detector_label: String,
    /// Colour names found in the crop, most dominant first.
    pub dominant_colors: Vec<String>,
    /// Titles the user already rejected for this item; do not propose them again.
    pub excluded_titles: Vec<String>,
    pub mode: ScanMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ProductGuess {
    pub title: String,
    #[uniffi(default = None)]
    pub brand: Option<String>,
    #[uniffi(default = None)]
    pub category: Option<String>,
    #[uniffi(default = [])]
    pub attributes: Vec<Attribute>,
    /// 0..1. Placeholder guesses report ~0.
    pub confidence: f32,
}

/// What a GTIN lookup returned.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LookupResult {
    #[uniffi(default = None)]
    pub title: Option<String>,
    #[uniffi(default = None)]
    pub brand: Option<String>,
    #[uniffi(default = None)]
    pub category: Option<String>,
    /// Where the data came from, for attribution ("Open Food Facts", ...).
    pub source: String,
}

// ───────────────────────────── catalog & listings ─────────────────────────────

/// One distinct product (SKU cluster) discovered during the scan.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct CatalogItem {
    pub id: String,
    /// Number of distinct physical units seen. An estimate, never an inventory count.
    pub quantity: u32,
    pub state: ItemState,
    pub role: Role,
    pub mode: ScanMode,
    pub guess: ProductGuess,
    pub guess_ready: bool,
    pub label: String,
    /// Validated GTIN (digits only) once a barcode was read or typed.
    pub gtin: Option<String>,
    pub sku: Option<String>,
    pub needs_barcode: bool,
    /// Best views as JPEG, best first. Not part of the persisted JSON (stored separately).
    #[serde(skip)]
    pub images: Vec<Vec<u8>>,
    pub first_seen_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ListingStatus {
    Draft,
    /// Present for completeness. The engine and every built-in storefront force `Draft`.
    Active,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ListingOption {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ListingVariant {
    /// One value per option, in option order.
    pub option_values: Vec<String>,
    #[uniffi(default = None)]
    pub sku: Option<String>,
    /// Decimal string, e.g. "19.99". Strings avoid float formatting bugs across languages.
    pub price: String,
    #[uniffi(default = None)]
    pub compare_at_price: Option<String>,
    pub quantity: u32,
    #[uniffi(default = None)]
    pub barcode: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ListingImage {
    pub jpeg: Vec<u8>,
    pub alt: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Listing {
    pub id: String,
    pub source_item_id: String,
    pub title: String,
    pub description_html: String,
    pub vendor: String,
    pub product_type: String,
    pub tags: Vec<String>,
    pub status: ListingStatus,
    pub options: Vec<ListingOption>,
    pub variants: Vec<ListingVariant>,
    pub images: Vec<ListingImage>,
    #[uniffi(default = None)]
    pub seo_title: Option<String>,
    #[uniffi(default = None)]
    pub seo_description: Option<String>,
    /// Confidence of the underlying product guess.
    pub confidence: f32,
    pub published: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PublishReceipt {
    pub listing_id: String,
    pub ok: bool,
    #[uniffi(default = None)]
    pub remote_id: Option<String>,
    #[uniffi(default = None)]
    pub url: Option<String>,
    pub message: String,
}

// ───────────────────────────── price advice ─────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct PriceRequest {
    pub item_id: String,
    pub title: String,
    #[uniffi(default = None)]
    pub gtin: Option<String>,
    #[uniffi(default = None)]
    pub category: Option<String>,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct PriceComp {
    pub title: String,
    pub price: String,
    pub currency: String,
    #[uniffi(default = None)]
    pub url: Option<String>,
    pub source: String,
    /// True for a sold price, false for an asking price.
    pub sold: bool,
}

/// A suggestion is only ever a range backed by the comps that produced it.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct PriceSuggestion {
    pub low: String,
    pub median: String,
    pub high: String,
    pub currency: String,
    pub comps: Vec<PriceComp>,
    pub note: String,
}

// ───────────────────────────── events & stats ─────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct OverlayBox {
    pub track_id: u64,
    pub bbox: BBox,
    #[uniffi(default = None)]
    pub item_id: Option<String>,
    #[uniffi(default = None)]
    pub state: Option<ItemState>,
    pub role: Role,
    /// Text to draw next to the box, e.g. "Looks like: red cereal box?".
    pub label: String,
    pub needs_barcode: bool,
    /// True for the box bound to the focused item.
    pub focused: bool,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ScanSummary {
    pub frames_in: u64,
    pub frames_dropped: u64,
    pub frames_skipped: u64,
    pub frames_processed: u64,
    pub detections: u64,
    pub tracks_promoted: u64,
    pub items: u32,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum Event {
    Stats { summary: ScanSummary, tracks_open: u32 },
    /// Per processed frame. Boxes are normalised; `hidden` counts Background/Excluded boxes left out.
    Overlay { frame_id: u64, timestamp_ms: u64, boxes: Vec<OverlayBox>, hidden: u32 },
    ItemDiscovered { item_id: String, quantity: u32 },
    ItemUpdated { item_id: String, quantity: u32 },
    ItemStateChanged { item_id: String, state: ItemState },
    GuessReady { item_id: String, title: String, confidence: f32 },
    /// Show the user "flip this over and show the barcode".
    BarcodeRequested { item_id: String, hint: String },
    BarcodeRead { item_id: String, gtin: String },
    /// A new item looks like one from an earlier session. Nothing was merged.
    PossibleDuplicate { item_id: String, existing_id: String, similarity: f32 },
    /// Two items turned out to be the same product (same GTIN); `from_id` no longer exists.
    ItemMerged { from_id: String, into_id: String },
    ScanFlushed { items: u32 },
    ListingDrafted { listing_id: String, title: String },
    PublishProgress { done: u32, total: u32 },
    Warning { source: String, message: String },
    Error { source: String, message: String },
}

/// Lock-free counters shared by the actors.
#[derive(Debug, Default)]
pub struct Stats {
    pub frames_in: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub frames_skipped: AtomicU64,
    pub frames_processed: AtomicU64,
    pub detections: AtomicU64,
    pub tracks_promoted: AtomicU64,
    pub items: AtomicU64,
}

impl Stats {
    pub fn snapshot(&self) -> ScanSummary {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        ScanSummary {
            frames_in: g(&self.frames_in),
            frames_dropped: g(&self.frames_dropped),
            frames_skipped: g(&self.frames_skipped),
            frames_processed: g(&self.frames_processed),
            detections: g(&self.detections),
            tracks_promoted: g(&self.tracks_promoted),
            items: g(&self.items) as u32,
        }
    }
    pub fn reset(&self) {
        for a in [&self.frames_in, &self.frames_dropped, &self.frames_skipped, &self.frames_processed, &self.detections, &self.tracks_promoted, &self.items] {
            a.store(0, Ordering::Relaxed);
        }
    }
}

// ───────────────────────────── config ─────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineConfig {
    // ── frame intake
    /// Frames are downscaled so the long side is at most this many pixels before detection.
    #[uniffi(default = 960)]
    pub analysis_max_side: u32,
    /// Admission rate for frames (the rest are dropped at the edge for free).
    #[uniffi(default = 6.0)]
    pub analysis_fps: f32,
    /// Mean absolute difference (0..255 scale) below which a frame counts as a repeat. 0 disables.
    #[uniffi(default = 0.6)]
    pub novelty_threshold: f32,
    /// Skip frames whose sharpness is below this fraction of the running median. 0 disables.
    #[uniffi(default = 0.25)]
    pub blur_gate_ratio: f32,
    // ── detection
    #[uniffi(default = 0.35)]
    pub min_score: f32,
    #[uniffi(default = 0.5)]
    pub nms_iou: f32,
    /// Boxes smaller than this fraction of the frame are ignored.
    #[uniffi(default = 0.002)]
    pub min_box_area: f32,
    /// Boxes larger than this fraction of the frame get the Background role (they are kept).
    #[uniffi(default = 0.5)]
    pub background_area: f32,
    #[uniffi(default = 300)]
    pub max_detections: u32,
    // ── tracking & clustering
    /// A track becomes a product (and is guessed) after this many sightings.
    #[uniffi(default = 3)]
    pub min_track_hits: u32,
    /// A track is closed after this many consecutive frames without a match.
    #[uniffi(default = 8)]
    pub max_track_age: u32,
    /// Cosine similarity at which two tracks are considered the same SKU.
    #[uniffi(default = 0.92)]
    pub sku_similarity: f32,
    /// Cosine similarity against the fixture library at which an item is hidden automatically.
    #[uniffi(default = 0.95)]
    pub fixture_similarity: f32,
    /// Cosine similarity against earlier sessions that triggers a PossibleDuplicate event.
    #[uniffi(default = 0.97)]
    pub duplicate_similarity: f32,
    #[uniffi(default = 4)]
    pub max_images_per_item: u32,
    /// If a detector returns masks, paste the product onto a white background.
    #[uniffi(default = true)]
    pub cutout_on_white: bool,
    #[uniffi(default = 85)]
    pub jpeg_quality: u8,
    // ── identity
    pub default_mode: ScanMode,
    /// How many identical, valid barcode reads are needed before a scan is accepted.
    #[uniffi(default = 2)]
    pub barcode_votes: u32,
    /// How many times a rejected guess is retried automatically.
    #[uniffi(default = 2)]
    pub guess_retries: u32,
    // ── listings
    #[uniffi(default = "SKU")]
    pub sku_prefix: String,
    #[uniffi(default = "0.00")]
    pub placeholder_price: String,
    #[uniffi(default = "USD")]
    pub currency: String,
    #[uniffi(default = "Unknown vendor")]
    pub default_vendor: String,
    /// Per-store voice for generated titles and descriptions (English).
    #[uniffi(default = "Clear, friendly, plain English. No hype. Do not invent specifications.")]
    pub style_prompt: String,
    #[uniffi(default = 10)]
    pub publish_chunk_size: u32,
    // ── runtime
    /// Tokio worker threads (async plumbing only; inference runs on the blocking pool).
    #[uniffi(default = 2)]
    pub worker_threads: u32,
    /// Size of the blocking pool used for plugin calls.
    #[uniffi(default = 8)]
    pub blocking_threads: u32,
    /// Frames allowed in flight. When none are free, new frames are dropped at the camera.
    #[uniffi(default = 4)]
    pub frame_credits: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            analysis_max_side: 960,
            analysis_fps: 6.0,
            novelty_threshold: 0.6,
            blur_gate_ratio: 0.25,
            min_score: 0.35,
            nms_iou: 0.5,
            min_box_area: 0.002,
            background_area: 0.5,
            max_detections: 300,
            min_track_hits: 3,
            max_track_age: 8,
            sku_similarity: 0.92,
            fixture_similarity: 0.95,
            duplicate_similarity: 0.97,
            max_images_per_item: 4,
            cutout_on_white: true,
            jpeg_quality: 85,
            default_mode: ScanMode::BarcodeFirst,
            barcode_votes: 2,
            guess_retries: 2,
            sku_prefix: "SKU".into(),
            placeholder_price: "0.00".into(),
            currency: "USD".into(),
            default_vendor: "Unknown vendor".into(),
            style_prompt: "Clear, friendly, plain English. No hype. Do not invent specifications.".into(),
            publish_chunk_size: 10,
            worker_threads: 2,
            blocking_threads: 8,
            frame_credits: 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iou_basics() {
        let a = BBox::new(0.0, 0.0, 0.5, 0.5);
        assert!((a.iou(&a) - 1.0).abs() < 1e-6);
        assert_eq!(a.iou(&BBox::new(0.6, 0.6, 0.2, 0.2)), 0.0);
        let b = BBox::new(0.25, 0.0, 0.5, 0.5);
        assert!((a.iou(&b) - (0.125 / 0.375)).abs() < 1e-5);
    }

    #[test]
    fn pixels_stay_inside() {
        let (x, y, w, h) = BBox::new(0.9, 0.9, 0.5, 0.5).to_pixels(100, 50);
        assert!(x + w <= 100 && y + h <= 50 && w >= 1 && h >= 1);
    }

    #[test]
    fn item_json_roundtrip_skips_images() {
        let item = CatalogItem {
            id: "item-001".into(),
            quantity: 2,
            state: ItemState::Confirmed { by: ConfirmedBy::Barcode },
            role: Role::Product,
            mode: ScanMode::BarcodeFirst,
            guess: ProductGuess { title: "Red box".into(), brand: None, category: None, attributes: vec![], confidence: 0.4 },
            guess_ready: true,
            label: "product".into(),
            gtin: Some("4006381333931".into()),
            sku: None,
            needs_barcode: false,
            images: vec![vec![1, 2, 3]],
            first_seen_ms: 5,
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(!json.contains("images"));
        let back: CatalogItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state, ItemState::Confirmed { by: ConfirmedBy::Barcode });
        assert!(back.images.is_empty());
    }
}
