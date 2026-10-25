//! `shelfscan fixtures --out DIR`: deterministic PNGs for tests in other languages (Swift, Kotlin, Python).
//!
//! ```text
//! DIR/scene-00.png .. scene-11.png    Scene::three_products(), 12 frames at 6 fps (3 products, well separated)
//! DIR/ean13.png                       a white EAN-13 card with quiet zones (module 3 px)
//! DIR/barcode-m{1..4}-b{0,1,2}.png    the card on a table at module 1..4 px ("distance") and blur sigma 0..2
//! DIR/fixtures.json                   manifest: files, GTIN, timestamps, expected product count
//! ```

use std::path::Path;

use anyhow::{Context, Result};
use image::{imageops, Rgb, RgbImage};
use serde_json::json;
use shelfscan::{synthetic, Frame};

pub const GTIN: &str = "4006381333931";
pub const SCENE_FRAMES: usize = 12;
pub const SCENE_FPS: f32 = 6.0;
const T0_MS: u64 = 1000;
const MODULES: [u32; 4] = [1, 2, 3, 4];
const BLURS: [f32; 3] = [0.0, 1.0, 2.0];

fn save(frame: &Frame, path: &Path) -> Result<()> {
    let img = RgbImage::from_raw(frame.width, frame.height, frame.data.clone()).context("synthetic frame is not packed RGB")?;
    img.save(path).with_context(|| format!("writing {}", path.display()))
}

/// The barcode card centred on a 640x480 table, optionally blurred.
fn barcode_on_table(module_px: u32, blur: f32) -> Result<RgbImage> {
    let card = synthetic::render_ean13(GTIN, module_px, 40 * module_px).context("bad GTIN")?;
    let (w, h) = (640u32, 480u32);
    let mut img = RgbImage::from_pixel(w, h, Rgb(synthetic::WOOD));
    imageops::overlay(&mut img, &card, ((w - card.width()) / 2) as i64, ((h - card.height()) / 2) as i64);
    Ok(if blur > 0.0 { imageops::blur(&img, blur) } else { img })
}

pub fn write(out: &Path) -> Result<()> {
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let scene = synthetic::Scene::three_products();
    let mut frames = vec![];
    for (i, f) in scene.frames(SCENE_FRAMES, SCENE_FPS, T0_MS, 7).iter().enumerate() {
        let name = format!("scene-{i:02}.png");
        save(f, &out.join(&name))?;
        frames.push(json!({ "file": name, "timestamp_ms": f.timestamp_ms }));
    }
    let card = synthetic::render_ean13(GTIN, 3, 120).context("bad GTIN")?;
    card.save(out.join("ean13.png"))?;
    let mut barcodes = vec![];
    for m in MODULES {
        for b in BLURS {
            let name = format!("barcode-m{m}-b{}.png", b as u32);
            barcode_on_table(m, b)?.save(out.join(&name))?;
            barcodes.push(json!({ "file": name, "module_px": m, "blur_sigma": b }));
        }
    }
    let manifest = json!({
        "scene": { "frames": frames, "fps": SCENE_FPS, "width": scene.width, "height": scene.height, "products": scene.objects.len() },
        "ean13": { "file": "ean13.png", "gtin": GTIN },
        "barcodes": { "gtin": GTIN, "frames": barcodes },
    });
    std::fs::write(out.join("fixtures.json"), serde_json::to_string_pretty(&manifest)?)?;
    eprintln!("{} fixture files written to {}", frames.len() + barcodes.len() + 2, out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_manifest_and_decodable_pngs() {
        let dir = std::env::temp_dir().join(format!("shelfscan-fixtures-{}", std::process::id()));
        write(&dir).unwrap();
        let m: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("fixtures.json")).unwrap()).unwrap();
        assert_eq!(m["scene"]["frames"].as_array().unwrap().len(), SCENE_FRAMES);
        assert_eq!(m["scene"]["products"], 3);
        assert_eq!(m["barcodes"]["frames"].as_array().unwrap().len(), MODULES.len() * BLURS.len());
        let first = image::open(dir.join("scene-00.png")).unwrap();
        assert_eq!((first.width(), first.height()), (480, 360));
        assert!(image::open(dir.join("barcode-m4-b2.png")).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
