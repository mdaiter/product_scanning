//! Storefront plugins. Every one of them forces `DRAFT`.

pub mod csv;
#[cfg(feature = "net")]
pub mod shopify;

pub use csv::CsvStorefront;
#[cfg(feature = "net")]
pub use shopify::ShopifyStorefront;
