use std::{collections::HashSet, sync::Arc};

use rxing::{BarcodeFormat, DecodeHints};

use crate::error::{Result, ShelfError};
use crate::imaging::frame_to_rgb;
use crate::traits::BarcodeScanner;
use crate::types::{BarcodeRead, Frame};

/// Retail barcode reader (EAN-13/8, UPC-A/E) on the pure-Rust ZXing port `rxing`. Works everywhere
/// with no native code. On a phone, prefer Vision (`VNDetectBarcodesRequest`) or ML Kit through the
/// `BarcodeScanner` trait: they are faster and read blurrier codes.
#[derive(uniffi::Object)]
pub struct RxingScanner;

#[uniffi::export]
impl RxingScanner {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl Default for RxingScanner {
    fn default() -> Self {
        Self
    }
}

fn symbology(f: &BarcodeFormat) -> String {
    match f {
        BarcodeFormat::EAN_13 => "EAN_13",
        BarcodeFormat::EAN_8 => "EAN_8",
        BarcodeFormat::UPC_A => "UPC_A",
        BarcodeFormat::UPC_E => "UPC_E",
        _ => "OTHER",
    }
    .to_string()
}

#[uniffi::export]
impl BarcodeScanner for RxingScanner {
    fn scan(&self, frame: Frame) -> Result<Vec<BarcodeRead>> {
        let img = frame_to_rgb(&frame)?;
        let (w, h) = img.dimensions();
        let luma = image::imageops::grayscale(&img).into_raw();
        let mut hints = DecodeHints {
            PossibleFormats: Some(HashSet::from([BarcodeFormat::EAN_13, BarcodeFormat::EAN_8, BarcodeFormat::UPC_A, BarcodeFormat::UPC_E])),
            TryHarder: Some(true),
            ..DecodeHints::default()
        };
        match rxing::helpers::detect_multiple_in_luma_with_hints(luma, w, h, &mut hints) {
            Ok(results) => Ok(results.iter().map(|r| BarcodeRead { text: r.getText().to_string(), symbology: symbology(r.getBarcodeFormat()) }).collect()),
            // "nothing found" is the normal case for most frames
            Err(e) if format!("{e:?}").contains("NotFound") => Ok(vec![]),
            Err(e) => Err(ShelfError::backend("rxing", format!("{e:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::{barcode_frame, render_ean13};

    #[test]
    fn reads_a_valid_ean13_from_a_camera_like_frame() {
        let f = barcode_frame("4006381333931", 640, 480, 1).unwrap();
        let reads = RxingScanner.scan(f).unwrap();
        assert!(reads.iter().any(|r| r.text == "4006381333931"), "{reads:?}");
        assert!(crate::gtin::is_valid(&reads[0].text));
    }

    #[test]
    fn reads_a_smaller_symbol_and_ignores_a_blank_frame() {
        let card = render_ean13("0036000291452", 2, 80).unwrap();
        let reads = RxingScanner.scan(crate::imaging::rgb_to_frame(&card, 0)).unwrap();
        assert!(reads.iter().any(|r| crate::gtin::canonical(&r.text) == crate::gtin::canonical("0036000291452")), "{reads:?}");

        let blank = crate::imaging::rgb_to_frame(&image::RgbImage::from_pixel(320, 240, image::Rgb([180, 150, 110])), 0);
        assert!(RxingScanner.scan(blank).unwrap().is_empty());
    }
}
