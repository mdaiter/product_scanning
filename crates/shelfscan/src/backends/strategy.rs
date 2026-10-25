//! How an item gets its identity. One trait, two strategies, chosen per session with a per-item
//! override. The Catalog asks the strategy; it never branches on the mode itself.

use crate::types::{CatalogItem, ScanMode};

pub trait IdentityStrategy: Send + Sync {
    fn mode(&self) -> ScanMode;
    /// After the first guess, should the user be asked to show the barcode?
    fn wants_barcode(&self) -> bool;
    /// Text for the "flip it over" prompt.
    fn barcode_hint(&self, item: &CatalogItem) -> String;
    /// How many additional views to send the guesser (besides the best crop).
    fn extra_views(&self) -> usize;
    /// What the user is asked to confirm.
    fn confirm_prompt(&self, item: &CatalogItem) -> String;
}

/// Catalog mode: the GTIN is ground truth, a visual guess is only a hint until the barcode is read.
pub struct BarcodeFirst;
/// Handmade / unbranded: describe from images, the user approves the text.
pub struct DescribeFirst;

impl IdentityStrategy for BarcodeFirst {
    fn mode(&self) -> ScanMode {
        ScanMode::BarcodeFirst
    }
    fn wants_barcode(&self) -> bool {
        true
    }
    fn barcode_hint(&self, item: &CatalogItem) -> String {
        format!("Flip \"{}\" over and hold the barcode up to the camera", item.guess.title)
    }
    fn extra_views(&self) -> usize {
        2
    }
    fn confirm_prompt(&self, item: &CatalogItem) -> String {
        format!("Looks like: {}?", item.guess.title)
    }
}

impl IdentityStrategy for DescribeFirst {
    fn mode(&self) -> ScanMode {
        ScanMode::DescribeFirst
    }
    fn wants_barcode(&self) -> bool {
        false
    }
    fn barcode_hint(&self, _item: &CatalogItem) -> String {
        String::new()
    }
    fn extra_views(&self) -> usize {
        3
    }
    fn confirm_prompt(&self, item: &CatalogItem) -> String {
        format!("Use this title? {}", item.guess.title)
    }
}

pub fn strategy_for(mode: ScanMode) -> &'static dyn IdentityStrategy {
    match mode {
        ScanMode::BarcodeFirst => &BarcodeFirst,
        ScanMode::DescribeFirst => &DescribeFirst,
    }
}
