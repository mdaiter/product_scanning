//! The extension points. Every trait is implementable from Rust *and* from Swift / Kotlin /
//! Python (`with_foreign`), which is how you plug in CoreML, Vision, LiteRT/ML Kit, a cloud
//! model, your own storefront, etc. All methods are synchronous: the engine calls them from its
//! blocking pool, so a slow model or HTTP call never stalls actor scheduling.

use crate::error::ShelfError;
use crate::types::*;

/// Finds products in a frame. Receives `Rgb8`, tightly packed, already downscaled.
///
/// This is where DETR / RT-DETR / RF-DETR-seg / Vision foreground masks live. On a phone,
/// implement it natively and hand it to the engine; the pipeline does not care where the tensors ran.
#[uniffi::export(with_foreign)]
pub trait Detector: Send + Sync {
    fn detect(&self, frame: Frame) -> Result<Vec<Detection>, ShelfError>;
}

/// Turns a product crop (`Rgb8`) into an appearance vector used to merge duplicate SKUs.
/// Vectors need not be normalised; the engine L2-normalises them.
#[uniffi::export(with_foreign)]
pub trait Embedder: Send + Sync {
    fn embed(&self, image: Frame) -> Result<Vec<f32>, ShelfError>;
}

/// Reads barcodes from a (high-resolution) frame. Return every valid-looking read; the engine
/// checks the GTIN check digit and requires repeated agreement before it trusts one.
#[uniffi::export(with_foreign)]
pub trait BarcodeScanner: Send + Sync {
    fn scan(&self, frame: Frame) -> Result<Vec<BarcodeRead>, ShelfError>;
}

/// Guesses what a product is. The built-in implementation only returns placeholders; implement
/// this with an on-device model, OCR + catalogue lookup, or a cloud model.
#[uniffi::export(with_foreign)]
pub trait ProductGuesser: Send + Sync {
    fn guess(&self, request: GuessRequest) -> Result<ProductGuess, ShelfError>;
}

/// Resolves a validated GTIN to product data (title, brand, category). `Ok(None)` = not found.
#[uniffi::export(with_foreign)]
pub trait ProductLookup: Send + Sync {
    fn lookup(&self, gtin: String) -> Result<Option<LookupResult>, ShelfError>;
}

/// Turns a catalog item into a listing draft. `style_prompt` is the per-store voice.
/// Unknown facts (size, material, condition) must be marked as needing the user's input,
/// never invented.
#[uniffi::export(with_foreign)]
pub trait ListingWriter: Send + Sync {
    fn write(&self, item: CatalogItem, style_prompt: String) -> Result<Listing, ShelfError>;
}

/// Looks up comparable prices. Only ever called when the user clicks "get price suggestions".
#[uniffi::export(with_foreign)]
pub trait PriceAdvisor: Send + Sync {
    fn suggest(&self, request: PriceRequest) -> Result<PriceSuggestion, ShelfError>;
}

/// A place listings can be sent to.
#[uniffi::export(with_foreign)]
pub trait Storefront: Send + Sync {
    fn name(&self) -> String;
    /// Publish a chunk of listings (always `Draft`). Return one receipt per listing.
    /// Return `Err` only if the whole chunk failed.
    fn publish(&self, listings: Vec<Listing>) -> Result<Vec<PublishReceipt>, ShelfError>;
}

/// Receives pipeline events. Called from a dedicated blocking thread, in order. Keep it quick or
/// hop to your UI thread.
#[uniffi::export(with_foreign)]
pub trait EventListener: Send + Sync {
    fn on_event(&self, event: Event);
}
