use crate::error::Result;
use crate::imaging::{downscale_max_side, frame_to_rgb};
use crate::traits::Detector;
use crate::types::{BBox, Detection, Frame, Mask};

/// Demo detector for a tabletop on a roughly uniform surface: estimates the background colour from
/// the frame border, thresholds the difference, closes small gaps and returns connected blobs with
/// masks. Products that touch each other merge into one blob, and a patterned table defeats it.
/// Replace it with a real model (RT-DETR / Vision foreground masks) for anything but demos.
#[derive(uniffi::Object)]
pub struct BlobDetector {
    work_side: u32,
    min_threshold: f32,
}

#[uniffi::export]
impl BlobDetector {
    #[uniffi::constructor]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    /// Tunable variant (e.g. from a settings screen). `work_side`: long side in pixels the frame is analysed at
    /// (clamped to 32..=1024); `min_threshold`: smallest colour difference from the border-median background that
    /// counts as foreground (clamped to 1..=255).
    #[uniffi::constructor]
    pub fn with_params(work_side: u32, min_threshold: f32) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { work_side: work_side.clamp(32, 1024), min_threshold: min_threshold.clamp(1.0, 255.0) })
    }
}

impl Default for BlobDetector {
    fn default() -> Self {
        Self { work_side: 160, min_threshold: 30.0 }
    }
}

fn median(v: &mut [u8]) -> u8 {
    v.sort_unstable();
    v[v.len() / 2]
}

fn morph(src: &[bool], w: usize, h: usize, dilate: bool) -> Vec<bool> {
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut hit = !dilate; // dilate: any neighbour set; erode: all neighbours set
            'n: for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    let v = if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        // outside the frame counts as background for dilate and as "set" for erode,
                        // so objects touching the edge are not eaten.
                        !dilate
                    } else {
                        src[ny as usize * w + nx as usize]
                    };
                    if dilate && v {
                        hit = true;
                        break 'n;
                    }
                    if !dilate && !v {
                        hit = false;
                        break 'n;
                    }
                }
            }
            out[y * w + x] = hit;
        }
    }
    out
}

#[uniffi::export]
impl Detector for BlobDetector {
    fn detect(&self, frame: Frame) -> Result<Vec<Detection>> {
        let img = downscale_max_side(frame_to_rgb(&frame)?, self.work_side);
        let (w, h) = (img.width() as usize, img.height() as usize);
        if w < 8 || h < 8 {
            return Ok(vec![]);
        }
        // Background colour = per-channel median of the border ring.
        let ring = (w.min(h) / 25).max(1);
        let on_ring = |x: usize, y: usize| x < ring || y < ring || x >= w - ring || y >= h - ring;
        let mut chans: [Vec<u8>; 3] = [vec![], vec![], vec![]];
        for y in 0..h {
            for x in 0..w {
                if on_ring(x, y) {
                    let p = img.get_pixel(x as u32, y as u32).0;
                    for c in 0..3 {
                        chans[c].push(p[c]);
                    }
                }
            }
        }
        let bg = [median(&mut chans[0]), median(&mut chans[1]), median(&mut chans[2])];
        let diff = |x: usize, y: usize| -> f32 {
            let p = img.get_pixel(x as u32, y as u32).0;
            (0..3).map(|c| (p[c] as i32 - bg[c] as i32).abs()).max().unwrap_or(0) as f32
        };
        // Noise level from the border itself, robust to products that touch the edge.
        let mut border: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| on_ring(x, y)).map(|(x, y)| diff(x, y) as u8).collect();
        let noise = median(&mut border) as f32;
        let thr = self.min_threshold.max(4.0 * noise);

        let mut fg = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                fg[y * w + x] = diff(x, y) > thr;
            }
        }
        let fg = morph(&morph(&fg, w, h, true), w, h, false); // close

        // Connected components (8-neighbourhood).
        let mut seen = vec![false; w * h];
        let mut out = Vec::new();
        let min_px = ((w * h) as f32 * 0.001).max(6.0) as usize;
        for sy in 0..h {
            for sx in 0..w {
                if !fg[sy * w + sx] || seen[sy * w + sx] {
                    continue;
                }
                let mut stack = vec![(sx, sy)];
                seen[sy * w + sx] = true;
                let mut pixels = Vec::new();
                let (mut x0, mut y0, mut x1, mut y1) = (sx, sy, sx, sy);
                while let Some((x, y)) = stack.pop() {
                    pixels.push((x, y));
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                                continue;
                            }
                            let i = ny as usize * w + nx as usize;
                            if fg[i] && !seen[i] {
                                seen[i] = true;
                                stack.push((nx as usize, ny as usize));
                            }
                        }
                    }
                }
                if pixels.len() < min_px {
                    continue;
                }
                let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
                let mut mask = vec![0u8; bw * bh];
                for &(x, y) in &pixels {
                    mask[(y - y0) * bw + (x - x0)] = 255;
                }
                let fill = pixels.len() as f32 / (bw * bh) as f32;
                out.push(Detection {
                    bbox: BBox::new(x0 as f32 / w as f32, y0 as f32 / h as f32, bw as f32 / w as f32, bh as f32 / h as f32),
                    score: (0.5 + 0.5 * fill).min(1.0),
                    label: "product".into(),
                    mask: Some(Mask { width: bw as u32, height: bh as u32, data: mask }),
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::{Lcg, Scene, BLUE, RED};

    fn detect(scene: &Scene, noise: u8) -> Vec<Detection> {
        let img = scene.render((0.0, 0.0), noise, &mut Lcg::new(3));
        BlobDetector::default().detect(crate::imaging::rgb_to_frame(&img, 0)).unwrap()
    }

    #[test]
    fn with_params_clamps_and_still_detects() {
        let d = BlobDetector::with_params(5, 1000.0);
        assert_eq!((d.work_side, d.min_threshold), (32, 255.0));
        let scene = Scene::three_products();
        let img = scene.render((0.0, 0.0), 0, &mut crate::synthetic::Lcg::new(1));
        let dets = BlobDetector::with_params(240, 30.0).detect(crate::imaging::rgb_to_frame(&img, 0)).unwrap();
        assert_eq!(dets.len(), 3);
    }

    #[test]
    fn finds_each_product_with_a_tight_box_and_mask() {
        let scene = Scene::three_products();
        let dets = detect(&scene, 4);
        assert_eq!(dets.len(), 3, "{dets:?}");
        for o in &scene.objects {
            let best = dets.iter().map(|d| d.bbox.iou(&o.bbox)).fold(0.0, f32::max);
            assert!(best > 0.85, "iou {best} for {:?}", o.bbox);
        }
        let m = dets[0].mask.as_ref().unwrap();
        assert!(m.data.iter().filter(|&&v| v == 255).count() as f32 > 0.8 * (m.width * m.height) as f32);
    }

    #[test]
    fn empty_table_has_no_detections() {
        assert!(detect(&Scene::empty(320, 240), 6).is_empty());
    }

    #[test]
    fn object_touching_the_edge_is_still_found() {
        let scene = Scene::empty(320, 240).with(BBox::new(0.0, 0.3, 0.25, 0.4), RED).with(BBox::new(0.6, 0.3, 0.2, 0.4), BLUE);
        assert_eq!(detect(&scene, 3).len(), 2);
    }
}
