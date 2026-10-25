//! shelfscan: scan a store with a camera, index products, draft listings, publish to a storefront.
//!
//! Architecture and decisions live in `PLAN.md` at the repository root.

uniffi::setup_scaffolding!();

pub mod backends;
pub mod engine;
pub mod error;
pub mod eventlog;
pub mod gtin;
pub mod imaging;
pub mod store;
pub mod synthetic;
pub mod traits;
pub mod types;

pub use error::ShelfError;
pub use traits::*;
pub use types::*;
pub mod actors;
#[cfg(feature = "net")]
pub mod cloud;
pub mod storefront;

pub use engine::{Engine, EngineBuilder};
pub use eventlog::EventLog;
