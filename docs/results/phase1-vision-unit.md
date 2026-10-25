# 1.3 Vision plugins: unit results on synthetic fixtures

Ran 2026-10-25 on the dev Mac (M3 Pro, macOS 26.3.1, Xcode 26.0.1), `bash scripts/swift_test.sh`, debug build.
Fixtures from `shelfscan fixtures` (deterministic; `crates/shelfscan-cli/src/fixtures.rs`). **Synthetic only.**
Real photos of barcodes and real frames are still to do (they need the 1.5 items); nothing here says how the plugins
behave on a webcam.

## Barcode (`VisionBarcodeScanner` vs `RxingScanner`)

EAN-13 `4006381333931` printed on a white card, centred on a 640x480 "table", module width 1 to 4 px, Gaussian blur sigma 0 to 2.

| module px | blur σ | Vision | rxing |
|---|---|---|---|
| 1 | 0 | yes | yes |
| 1 | 1 | no | no |
| 1 | 2 | no | no |
| 2 | 0 | yes | yes |
| 2 | 1 | yes | yes |
| 2 | 2 | no | no |
| 3 | 0 | yes | yes |
| 3 | 1 | yes | yes |
| 3 | 2 | no | no |
| 4 | 0 | yes | yes |
| 4 | 1 | yes | yes |
| 4 | 2 | **yes** | no |

* Decoded 8/12 (Vision) vs 7/12 (rxing). Neither produced a wrong GTIN.
* Time per 640x480 frame: Vision ~6 ms warm, **~900 ms on the first call** (model load); rxing 3 to 12 ms.
  The first-call cost matters for focus mode: warm the Vision scanner up when the engine starts.
* Standalone card (`ean13.png`) decodes with Vision.

## Embedder (`VisionFeaturePrintEmbedder`)

* Vector length **768** floats, L2 norm 1.0000 after the plugin's normalisation, same crop twice: cosine 1.0000.
* Same synthetic product in frames 0 and 5 (shake + noise): cosine **0.949**. Red vs blue product: **0.858**.
  The gap is narrow on flat colour blocks; the default `sku_similarity` 0.92 sits between them here, which says nothing
  about real products. Decision belongs to the 1.8 bake-off.

## Detector (`VisionForegroundDetector`, `VNGenerateForegroundInstanceMaskRequest`)

* Scene frame 0 (480x360, three products at different heights): 3 detections, IoU with truth 0.953 / 0.946 / 0.962.
* **Mask buffer origin: top-left, aligned with the input** (hypothesis in the sketch confirmed on synthetic frames;
  a bottom-left origin would have put boxes at the wrong heights). Mask centre pixels are set; mask size = box size.
* `.bgra8` frame with padded rows (stride = width*4 + 64, as the camera delivers): same boxes, both for Vision
  (`Frame.cgImage()`) and for the Rust `BlobDetector`.
* `BlobDetector` on the same frame: IoU 0.936 / 0.940 / 0.945.
* Vision ~0.3 s for the first detection call (includes model load); per-frame timing under load is part of the 1.7 bake-off.
