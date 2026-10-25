use std::sync::Arc;

use crate::error::Result;
use crate::imaging::{color_signature, frame_to_rgb};
use crate::traits::Embedder;
use crate::types::Frame;

/// Spatial colour-histogram embedder. Deterministic and dependency-free, but it only sees colour:
/// two differently shaped products of the same colour will look alike. Use a learned embedder
/// (Vision feature print, CLIP/SigLIP via CoreML, DINOv2) for real catalogues, and keep this one
/// as the fallback and the test double.
#[derive(uniffi::Object)]
pub struct ColorEmbedder {
    grid: u32,
}

#[uniffi::export]
impl ColorEmbedder {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { grid: 3 })
    }
}

impl Default for ColorEmbedder {
    fn default() -> Self {
        Self { grid: 3 }
    }
}

#[uniffi::export]
impl Embedder for ColorEmbedder {
    fn embed(&self, image: Frame) -> Result<Vec<f32>> {
        Ok(color_signature(&frame_to_rgb(&image)?, self.grid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imaging::{cosine, rgb_to_frame};
    use crate::synthetic::{BLUE, RED};

    fn solid(c: [u8; 3]) -> Frame {
        rgb_to_frame(&image::RgbImage::from_pixel(50, 70, image::Rgb(c)), 0)
    }

    #[test]
    fn same_colour_similar_different_colour_not() {
        let e = ColorEmbedder::default();
        let (a, b, c) = (e.embed(solid(RED)).unwrap(), e.embed(solid([205, 35, 45])).unwrap(), e.embed(solid(BLUE)).unwrap());
        assert!(cosine(&a, &b) > 0.95);
        assert!(cosine(&a, &c) < 0.3);
    }
}
