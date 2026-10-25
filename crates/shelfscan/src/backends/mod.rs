//! Built-in plugin implementations. They are deliberately simple: they exist so the whole
//! pipeline runs anywhere with zero native dependencies, and as reference code for real plugins.

pub mod blob_detector;
pub mod color_embedder;
pub mod placeholder;
pub mod price;
pub mod strategy;
#[cfg(feature = "barcode")]
pub mod rxing_scanner;

pub use blob_detector::BlobDetector;
pub use color_embedder::ColorEmbedder;
pub use placeholder::{PlaceholderGuesser, TemplateWriter};
pub use price::StubPriceAdvisor;
#[cfg(feature = "barcode")]
pub use rxing_scanner::RxingScanner;
