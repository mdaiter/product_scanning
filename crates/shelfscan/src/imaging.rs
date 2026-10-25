//! Pixel plumbing: format conversion, crops, quality scoring, JPEG, colour signatures.
//! Pure Rust, no SIMD tricks, no platform code: it runs the same everywhere.

use crate::error::{Result, ShelfError};
use crate::types::{BBox, Frame, Mask, PixelFormat};
use image::{imageops::FilterType, RgbImage};

// ───────────────────────────── Frame <-> RgbImage ─────────────────────────────

pub fn frame_to_rgb(f: &Frame) -> Result<RgbImage> {
    let (w, h) = (f.width as usize, f.height as usize);
    if w == 0 || h == 0 {
        return Err(ShelfError::invalid("frame has zero size"));
    }
    let bpp = match f.format {
        PixelFormat::Rgb8 => 3,
        PixelFormat::Rgba8 | PixelFormat::Bgra8 => 4,
        PixelFormat::Gray8 | PixelFormat::Nv12 => 1,
    };
    let stride = if f.stride == 0 { w * bpp } else { f.stride as usize };
    if stride < w * bpp {
        return Err(ShelfError::invalid(format!("stride {stride} smaller than row size {}", w * bpp)));
    }
    let need = match f.format {
        PixelFormat::Nv12 => stride * h + stride * h.div_ceil(2),
        _ => stride * (h - 1) + w * bpp,
    };
    if f.data.len() < need {
        return Err(ShelfError::invalid(format!("frame data too short: have {}, need {need}", f.data.len())));
    }
    let d = &f.data;
    let mut out = vec![0u8; w * h * 3];
    match f.format {
        PixelFormat::Rgb8 => {
            for y in 0..h {
                out[y * w * 3..(y + 1) * w * 3].copy_from_slice(&d[y * stride..y * stride + w * 3]);
            }
        }
        PixelFormat::Rgba8 | PixelFormat::Bgra8 => {
            let bgr = f.format == PixelFormat::Bgra8;
            for y in 0..h {
                let row = &d[y * stride..y * stride + w * 4];
                let o = &mut out[y * w * 3..(y + 1) * w * 3];
                for x in 0..w {
                    let (a, b, c) = (row[x * 4], row[x * 4 + 1], row[x * 4 + 2]);
                    let (r, g, bl) = if bgr { (c, b, a) } else { (a, b, c) };
                    o[x * 3] = r;
                    o[x * 3 + 1] = g;
                    o[x * 3 + 2] = bl;
                }
            }
        }
        PixelFormat::Gray8 => {
            for y in 0..h {
                let row = &d[y * stride..y * stride + w];
                let o = &mut out[y * w * 3..(y + 1) * w * 3];
                for x in 0..w {
                    let v = row[x];
                    o[x * 3] = v;
                    o[x * 3 + 1] = v;
                    o[x * 3 + 2] = v;
                }
            }
        }
        PixelFormat::Nv12 => {
            let uv_base = stride * h;
            for y in 0..h {
                let yrow = &d[y * stride..y * stride + w];
                let uvrow = &d[uv_base + (y / 2) * stride..];
                let o = &mut out[y * w * 3..(y + 1) * w * 3];
                for x in 0..w {
                    let yy = yrow[x] as i32;
                    let u = uvrow[(x / 2) * 2] as i32 - 128;
                    let v = uvrow[(x / 2) * 2 + 1] as i32 - 128;
                    // BT.601 limited range, fixed point.
                    let c = 298 * (yy - 16);
                    o[x * 3] = ((c + 409 * v + 128) >> 8).clamp(0, 255) as u8;
                    o[x * 3 + 1] = ((c - 100 * u - 208 * v + 128) >> 8).clamp(0, 255) as u8;
                    o[x * 3 + 2] = ((c + 516 * u + 128) >> 8).clamp(0, 255) as u8;
                }
            }
        }
    }
    RgbImage::from_raw(f.width, f.height, out).ok_or_else(|| ShelfError::internal("rgb buffer size mismatch"))
}

/// Wrap an RGB image as a tightly packed `Rgb8` frame (copies the pixels).
pub fn rgb_to_frame(img: &RgbImage, timestamp_ms: u64) -> Frame {
    Frame { width: img.width(), height: img.height(), format: PixelFormat::Rgb8, data: img.as_raw().clone(), stride: 0, timestamp_ms }
}

pub fn downscale_max_side(img: RgbImage, max_side: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if max_side == 0 || long <= max_side {
        return img;
    }
    let s = max_side as f32 / long as f32;
    let nw = ((w as f32 * s).round() as u32).max(1);
    let nh = ((h as f32 * s).round() as u32).max(1);
    image::imageops::resize(&img, nw, nh, FilterType::Triangle)
}

pub fn crop(img: &RgbImage, bbox: &BBox, pad: f32) -> RgbImage {
    let padded = BBox::new(bbox.x - bbox.w * pad, bbox.y - bbox.h * pad, bbox.w * (1.0 + 2.0 * pad), bbox.h * (1.0 + 2.0 * pad));
    let (x, y, w, h) = padded.to_pixels(img.width(), img.height());
    image::imageops::crop_imm(img, x, y, w, h).to_image()
}

/// Composite `crop` (which must be the exact bbox crop) onto white using the detector's mask.
pub fn cutout_on_white(crop: &RgbImage, mask: &Mask) -> RgbImage {
    if mask.width == 0 || mask.height == 0 || mask.data.len() < (mask.width * mask.height) as usize {
        return crop.clone();
    }
    let mut out = crop.clone();
    let (cw, ch) = crop.dimensions();
    for y in 0..ch {
        let my = (y * mask.height / ch).min(mask.height - 1);
        for x in 0..cw {
            let mx = (x * mask.width / cw).min(mask.width - 1);
            if mask.data[(my * mask.width + mx) as usize] < 128 {
                out.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
    }
    out
}

pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(32 * 1024);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100))
        .encode_image(img)
        .map_err(|e| ShelfError::internal(format!("jpeg encode: {e}")))?;
    Ok(buf)
}

// ───────────────────────────── quality ─────────────────────────────

/// Variance of the Laplacian on a ≤128px grayscale copy: higher = sharper.
pub fn sharpness(img: &RgbImage) -> f32 {
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 {
        return 0.0;
    }
    let small;
    let src = if w.max(h) > 128 {
        let s = 128.0 / w.max(h) as f32;
        small = image::imageops::resize(img, ((w as f32 * s) as u32).max(3), ((h as f32 * s) as u32).max(3), FilterType::Triangle);
        &small
    } else {
        img
    };
    let (w, h) = src.dimensions();
    let g = |x: u32, y: u32| -> f32 {
        let p = src.get_pixel(x, y).0;
        0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
    };
    let (mut sum, mut sum2, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let l = (g(x - 1, y) + g(x + 1, y) + g(x, y - 1) + g(x, y + 1) - 4.0 * g(x, y)) as f64;
            sum += l;
            sum2 += l * l;
            n += 1.0;
        }
    }
    if n == 0.0 {
        return 0.0;
    }
    let mean = sum / n;
    ((sum2 / n) - mean * mean).max(0.0) as f32
}

/// Higher is better: sharp, large, and not cut off by the frame edge.
pub fn crop_quality(crop: &RgbImage, truncated: bool) -> f32 {
    let area = (crop.width() * crop.height()) as f32;
    let q = (sharpness(crop) + 1.0).sqrt() * area.sqrt();
    if truncated { q * 0.6 } else { q }
}

// ───────────────────────────── colour ─────────────────────────────

pub fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

const HUE_BINS: usize = 12;
const SAT_BINS: usize = 2;
const GRAY_BINS: usize = 4; // black, dark gray, light gray, white
const BINS: usize = HUE_BINS * SAT_BINS + GRAY_BINS;

fn color_bin(r: u8, g: u8, b: u8) -> usize {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    if s < 0.18 || v < 0.15 {
        let idx = if v < 0.15 { 0 } else if v < 0.5 { 1 } else if v < 0.85 { 2 } else { 3 };
        HUE_BINS * SAT_BINS + idx
    } else {
        let hb = ((h / 360.0 * HUE_BINS as f32) as usize).min(HUE_BINS - 1);
        let sb = if s < 0.55 { 0 } else { 1 };
        hb * SAT_BINS + sb
    }
}

/// Spatial colour histogram: `grid x grid` cells x 28 bins, L2-normalised. Cheap, deterministic,
/// good enough to tell a red box from a blue bottle and to track an object between frames.
pub fn color_signature(img: &RgbImage, grid: u32) -> Vec<f32> {
    let grid = grid.max(1);
    let (w, h) = img.dimensions();
    let mut v = vec![0f32; BINS * (grid * grid) as usize];
    let step_x = (w / 48).max(1);
    let step_y = (h / 48).max(1);
    let mut y = 0;
    while y < h {
        let gy = (y * grid / h).min(grid - 1);
        let mut x = 0;
        while x < w {
            let gx = (x * grid / w).min(grid - 1);
            let p = img.get_pixel(x, y).0;
            v[((gy * grid + gx) as usize) * BINS + color_bin(p[0], p[1], p[2])] += 1.0;
            x += step_x;
        }
        y += step_y;
    }
    l2_normalize(&mut v);
    v
}

const NAMED: &[(&str, [u8; 3])] = &[
    ("red", [200, 30, 40]),
    ("orange", [240, 130, 30]),
    ("yellow", [240, 210, 40]),
    ("green", [50, 160, 70]),
    ("teal", [30, 150, 150]),
    ("blue", [40, 80, 200]),
    ("purple", [130, 60, 170]),
    ("pink", [240, 130, 180]),
    ("brown", [120, 80, 50]),
    ("white", [245, 245, 245]),
    ("gray", [128, 128, 128]),
    ("black", [20, 20, 20]),
];

/// Up to `n` colour names ordered by how much of the crop they cover.
pub fn dominant_colors(img: &RgbImage, n: usize) -> Vec<String> {
    let mut counts = vec![0u32; NAMED.len()];
    let step_x = (img.width() / 48).max(1);
    let step_y = (img.height() / 48).max(1);
    let mut y = 0;
    while y < img.height() {
        let mut x = 0;
        while x < img.width() {
            let p = img.get_pixel(x, y).0;
            let best = NAMED
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, c))| {
                    let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
                    d(p[0], c[0]) + d(p[1], c[1]) + d(p[2], c[2])
                })
                .map(|(i, _)| i)
                .unwrap_or(0);
            counts[best] += 1;
            x += step_x;
        }
        y += step_y;
    }
    let mut idx: Vec<usize> = (0..NAMED.len()).filter(|&i| counts[i] > 0).collect();
    idx.sort_by_key(|&i| std::cmp::Reverse(counts[i]));
    idx.into_iter().take(n).map(|i| NAMED[i].0.to_string()).collect()
}

// ───────────────────────────── vectors ─────────────────────────────

pub fn l2_normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-12 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Cosine similarity; vectors of different length compare as 0.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 1e-12 || nb <= 1e-12 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, image::Rgb(c))
    }

    #[test]
    fn nv12_gray_converts_to_gray() {
        let (w, h) = (4u32, 4u32);
        let mut data = vec![126u8; (w * h) as usize]; // mid luma
        data.extend(std::iter::repeat(128u8).take((w * h / 2) as usize)); // neutral chroma
        let f = Frame { width: w, height: h, format: PixelFormat::Nv12, data, stride: 0, timestamp_ms: 0 };
        let img = frame_to_rgb(&f).unwrap();
        let p = img.get_pixel(1, 1).0;
        assert!((p[0] as i32 - p[1] as i32).abs() <= 2 && (p[1] as i32 - p[2] as i32).abs() <= 2);
        assert!(p[0] > 120 && p[0] < 150, "{p:?}");
    }

    #[test]
    fn bgra_swaps_channels_and_respects_stride() {
        // 2x1 image, stride padded to 12 bytes. Pixel0 = B,G,R,A = (0,0,255,255) = red.
        let data = vec![0, 0, 255, 255, 0, 255, 0, 255, 9, 9, 9, 9];
        let f = Frame { width: 2, height: 1, format: PixelFormat::Bgra8, data, stride: 12, timestamp_ms: 0 };
        let img = frame_to_rgb(&f).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 255, 0]);
    }

    #[test]
    fn short_buffer_is_rejected() {
        let f = Frame { width: 10, height: 10, format: PixelFormat::Rgb8, data: vec![0; 10], stride: 0, timestamp_ms: 0 };
        assert!(frame_to_rgb(&f).is_err());
    }

    #[test]
    fn signature_separates_colors() {
        let red = color_signature(&solid(60, 60, [200, 20, 20]), 2);
        let red2 = color_signature(&solid(80, 50, [210, 25, 25]), 2);
        let blue = color_signature(&solid(60, 60, [20, 20, 200]), 2);
        assert!(cosine(&red, &red2) > 0.95);
        assert!(cosine(&red, &blue) < 0.2);
    }

    #[test]
    fn names_colors() {
        assert_eq!(dominant_colors(&solid(40, 40, [210, 30, 40]), 1), vec!["red"]);
        assert_eq!(dominant_colors(&solid(40, 40, [45, 85, 205]), 1), vec!["blue"]);
    }

    #[test]
    fn sharp_beats_blurry() {
        let mut checker = RgbImage::new(64, 64);
        for (x, y, p) in checker.enumerate_pixels_mut() {
            *p = if (x / 4 + y / 4) % 2 == 0 { image::Rgb([255; 3]) } else { image::Rgb([0; 3]) };
        }
        let blurry = image::imageops::blur(&checker, 4.0);
        assert!(sharpness(&checker) > sharpness(&blurry) * 2.0);
    }

    #[test]
    fn jpeg_roundtrip() {
        let j = encode_jpeg(&solid(32, 32, [10, 200, 30]), 90).unwrap();
        let back = image::load_from_memory(&j).unwrap().to_rgb8();
        assert_eq!(back.dimensions(), (32, 32));
    }
}
