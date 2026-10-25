//! Synthetic tabletop scenes: coloured boxes on a table, camera shake, sensor noise, EAN-13
//! barcodes. Deterministic, so pipeline tests and the demo run anywhere without a camera.

use image::{Rgb, RgbImage};

use crate::imaging::rgb_to_frame;
use crate::types::{BBox, Frame};

#[derive(Debug, Clone)]
pub struct SceneObject {
    pub bbox: BBox,
    pub color: [u8; 3],
    /// Optional second colour drawn as a horizontal band across the middle (a "label").
    pub band: Option<[u8; 3]>,
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub table: [u8; 3],
    pub objects: Vec<SceneObject>,
}

/// Tiny deterministic generator (xorshift) so tests never depend on a rand crate.
pub struct Lcg(u64);
impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    /// Uniform in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

pub const RED: [u8; 3] = [200, 30, 40];
pub const BLUE: [u8; 3] = [40, 80, 200];
pub const GREEN: [u8; 3] = [50, 160, 70];
pub const YELLOW: [u8; 3] = [240, 210, 40];
pub const PURPLE: [u8; 3] = [130, 60, 170];
pub const WOOD: [u8; 3] = [196, 164, 120];
pub const BLACK: [u8; 3] = [20, 20, 20];

impl Scene {
    /// A bare table.
    pub fn empty(width: u32, height: u32) -> Self {
        Self { width, height, table: WOOD, objects: vec![] }
    }

    pub fn with(mut self, bbox: BBox, color: [u8; 3]) -> Self {
        self.objects.push(SceneObject { bbox, color, band: None });
        self
    }

    pub fn with_band(mut self, bbox: BBox, color: [u8; 3], band: [u8; 3]) -> Self {
        self.objects.push(SceneObject { bbox, color, band: Some(band) });
        self
    }

    /// Three well separated products on a table: red, blue, green.
    pub fn three_products() -> Self {
        Self::empty(480, 360)
            .with(BBox::new(0.08, 0.20, 0.20, 0.35), RED)
            .with(BBox::new(0.40, 0.25, 0.18, 0.40), BLUE)
            .with(BBox::new(0.70, 0.18, 0.20, 0.38), GREEN)
    }

    /// `shift` moves everything by a fraction of the frame (camera shake), `noise` is +-amplitude per channel.
    pub fn render(&self, shift: (f32, f32), noise: u8, rng: &mut Lcg) -> RgbImage {
        let mut img = RgbImage::from_pixel(self.width, self.height, Rgb(self.table));
        for o in &self.objects {
            let b = BBox::new(o.bbox.x + shift.0, o.bbox.y + shift.1, o.bbox.w, o.bbox.h);
            let (x0, y0, w, h) = b.to_pixels(self.width, self.height);
            for y in y0..(y0 + h).min(self.height) {
                for x in x0..(x0 + w).min(self.width) {
                    let in_band = o.band.is_some() && y >= y0 + h * 2 / 5 && y < y0 + h * 3 / 5;
                    img.put_pixel(x, y, Rgb(if in_band { o.band.unwrap() } else { o.color }));
                }
            }
        }
        if noise > 0 {
            for p in img.pixels_mut() {
                for c in 0..3 {
                    let d = (rng.signed() * noise as f32) as i32;
                    p.0[c] = (p.0[c] as i32 + d).clamp(0, 255) as u8;
                }
            }
        }
        img
    }

    /// `n` frames at `fps` with small camera shake and noise, starting at `t0_ms`.
    pub fn frames(&self, n: usize, fps: f32, t0_ms: u64, seed: u64) -> Vec<Frame> {
        let mut rng = Lcg::new(seed);
        (0..n)
            .map(|i| {
                let shift = (rng.signed() * 0.004, rng.signed() * 0.004);
                let img = self.render(shift, 4, &mut rng);
                rgb_to_frame(&img, t0_ms + (i as f32 * 1000.0 / fps) as u64)
            })
            .collect()
    }
}

// ───────────────────────────── EAN-13 ─────────────────────────────

const L: [&str; 10] = ["0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011", "0110111", "0001011"];
const PARITY: [&str; 10] = ["LLLLLL", "LLGLGG", "LLGGLG", "LLGGGL", "LGLLGG", "LGGLLG", "LGGGLL", "LGLGLG", "LGLGGL", "LGGLGL"];

fn complement(code: &str) -> String {
    code.chars().map(|c| if c == '0' { '1' } else { '0' }).collect()
}
fn reversed(code: &str) -> String {
    code.chars().rev().collect()
}

/// The 95 modules of an EAN-13 symbol (`true` = bar). `gtin13` must be 13 digits.
pub fn ean13_modules(gtin13: &str) -> Option<Vec<bool>> {
    let d: Vec<usize> = gtin13.bytes().map(|b| (b as char).to_digit(10).map(|x| x as usize)).collect::<Option<_>>()?;
    if d.len() != 13 {
        return None;
    }
    let mut bits = String::from("101");
    for (i, &digit) in d[1..7].iter().enumerate() {
        let l = L[digit];
        match PARITY[d[0]].as_bytes()[i] {
            b'L' => bits.push_str(l),
            _ => bits.push_str(&reversed(&complement(l))), // G = reversed R = reversed complement of L
        }
    }
    bits.push_str("01010");
    for &digit in &d[7..13] {
        bits.push_str(&complement(L[digit]));
    }
    bits.push_str("101");
    Some(bits.chars().map(|c| c == '1').collect())
}

/// A white card with the barcode printed on it (quiet zones included).
pub fn render_ean13(gtin13: &str, module_px: u32, height: u32) -> Option<RgbImage> {
    let modules = ean13_modules(gtin13)?;
    let quiet = 12;
    let w = (modules.len() as u32 + 2 * quiet) * module_px;
    let mut img = RgbImage::from_pixel(w, height, Rgb([255, 255, 255]));
    for (i, &bar) in modules.iter().enumerate() {
        if bar {
            for dx in 0..module_px {
                for y in height / 8..height - height / 8 {
                    img.put_pixel((quiet + i as u32) * module_px + dx, y, Rgb([0, 0, 0]));
                }
            }
        }
    }
    Some(img)
}

/// A frame as a phone would see it when the user flips a product and shows the barcode:
/// the card centred on the table.
pub fn barcode_frame(gtin13: &str, width: u32, height: u32, t_ms: u64) -> Option<Frame> {
    let card = render_ean13(gtin13, 3, 120)?;
    let mut img = RgbImage::from_pixel(width, height, Rgb(WOOD));
    let (ox, oy) = ((width.saturating_sub(card.width())) / 2, (height.saturating_sub(card.height())) / 2);
    image::imageops::overlay(&mut img, &card, ox as i64, oy as i64);
    Some(rgb_to_frame(&img, t_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ean13_has_95_modules_and_guard_bars() {
        let m = ean13_modules("4006381333931").unwrap();
        assert_eq!(m.len(), 95);
        assert_eq!(&m[..3], &[true, false, true]);
        assert_eq!(&m[45..50], &[false, true, false, true, false]);
        assert_eq!(&m[92..], &[true, false, true]);
        assert!(ean13_modules("123").is_none());
    }

    #[test]
    fn frames_are_deterministic() {
        let a = Scene::three_products().frames(3, 6.0, 0, 7);
        let b = Scene::three_products().frames(3, 6.0, 0, 7);
        assert_eq!(a[2].data, b[2].data);
        assert_eq!(a[1].timestamp_ms, 166);
    }
}
