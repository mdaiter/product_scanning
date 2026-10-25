//! Optional network plugins (feature `net`). Nothing here runs unless the app constructs it.
//!
//! * [`anthropic`]: reference `ProductGuesser` and `ListingWriter` on the Anthropic Messages API.
//!   They only ever receive masked product crops (that is all the pipeline has), never full frames.
//! * [`off`]: GTIN → product data from Open Food Facts (ODbL: attribute and share-alike).
//! * [`http`]: a small blocking client with timeouts, retries and key-safe errors.

pub mod anthropic;
pub mod http;
pub mod off;

#[cfg(test)]
pub mod mock;
