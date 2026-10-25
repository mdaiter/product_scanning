# Phase 1: Mac. Real perception, a live preview, calibrated clustering

**Goal.** Replace the demo plugins with Apple's, see the pipeline live on a Mac webcam, and measure which detector and
embedder actually separate 10 mixed tabletop items. Everything is decided by numbers from real footage, not by taste.

**Gate in.** On the Mac, `cargo test --workspace --all-features` and `scripts/python_smoke.sh` pass (HANDOFF.md, "First 30 minutes").

**Gate out.** The acceptance tests at the bottom. Then update PLAN.md section 10 with what is now verified, and stop for Matt's review before Phase 2.

## Prerequisites

* Apple Silicon Mac, current Xcode, `rustup`, `brew install ffmpeg`.
* A webcam (built-in is fine; an iPhone as Continuity Camera also shows up as an `AVCaptureDevice`, **UNVERIFIED** for this app).
* From Matt: **10 physical items** for the test table: a mix of branded packaged goods with barcodes, a few unbranded or
  handmade things, at least 2 items present twice or three times (for quantity), at least 2 pairs of same-colour but different products.
  Plain table, one desk lamp. No people in frame.

## Work items

### 1.1 Baseline on the Mac
Run the 30-minute checklist in HANDOFF.md. Record versions and results in PLAN.md section 10.

### 1.2 Build the Swift package
`scripts/build_xcframework.sh` (never run on a Mac) then `cd apple && swift build`. Fix what breaks: Rust cross-compile for
`aarch64-apple-ios`, `-ios-sim`, `-darwin` (`ring`, bundled SQLite and `rxing` are the likely suspects), the module map name
(`shelfscanFFI`, plain `module`, not `framework module`), and Swift 6 concurrency warnings from the generated file.
Add `apple/Tests/ShelfScanTests`: build an `Engine` in Swift with a Swift `EventListener`, push one rendered synthetic frame
(generate fixtures with a small `shelfscan-cli fixtures` subcommand you add), and assert `ItemDiscovered` arrives.
*Done when:* `swift build` and `swift test` pass for macOS.

### 1.3 Compile and unit-test the Vision plugins
Files exist as **uncompiled sketches** in `apple/Sources/ShelfScanVision` (barcode scanner, feature-print embedder,
foreground-instance detector). Make them compile, then test each on fixtures:
* barcode: a rendered EAN-13 PNG (from the fixtures subcommand), plus 5 real photos of barcodes, decode rate per distance and blur;
* embedder: vector length, L2 norm, same image twice gives cosine ~1.0;
* detector: boxes and masks on the synthetic scene, then on real frames (is the mask buffer top-left origin and aligned with the input image? the sketch assumes yes).

### 1.4 Live Mac demo app (the "live output" the user asked for)
`apple/Examples/MacDemo` (SwiftUI): `CameraSource` -> `Engine`; `AVCaptureVideoPreviewLayer` (aspect-fit) with `OverlayView` on
top; a side list of items (crop, state, guess, quantity); buttons for confirm / reject / type a title / hide / focus an item;
a stats strip (frames in, dropped, skipped, items); "Draft listings" and "Export CSV". Read `docs/events.md` first: results come
back as events, the listener runs on an engine thread, `Overlay` is lossy.
Wire the plugins by name so you can switch them from a menu: Blob / Vision-foreground detector, Color / Vision-feature-print embedder, rxing / Vision barcode.
*Done when:* boxes track items on screen, the counters move, nothing blocks the UI, and the p50 / p95 glass-to-box latency is logged (`docs/events.md`, "Measuring latency").

### 1.5 Capture the tabletop dataset
Three sessions of about 60 s each with the same 10 items: A good light; B warm / dim light; C clutter plus background decor
(a plant, a mug that is not for sale). Record raw video (QuickTime or `ffmpeg`) and write ground truth to
`data/tabletop/truth.json`: `[{ "name": ..., "gtin": null | "...", "quantity": n, "same_colour_as": [names] }]`.
`data/` is git-ignored: videos stay local (size, and faces can end up in frame).

### 1.6 Deterministic replay
Live capture drops frames when the pipeline is busy, which makes comparisons noisy. (File inputs already run at real-time speed via `ffmpeg -re`, because timestamps come from the wall clock; without it the fps gate admits one frame of a fast-decoded file.) Add an offline mode for file sources
(`--source ffmpeg --offline` or a new `--source video`): wait for a credit instead of dropping, as the `images` source does.
Add `shelfscan eval` (or a test binary) that replays a video and prints SKUs found, quantities, merges, splits against `truth.json`.

### 1.7 Detector bake-off
Candidates: `BlobDetector`, Vision foreground instances, optionally one CoreML detector (RT-DETR / RF-DETR class; check the licence before bundling anything).
Metrics per session: product recall, touching-items merged into one box, background objects reported as products, ms per frame, frames per second sustained.
Output: a table in `docs/results/phase1-detector.md`.

### 1.8 Embedder bake-off (the decision that matters most)
Candidates: `ColorEmbedder` (baseline), Vision feature print, one CLIP/SigLIP-class CoreML model, DINOv2-small CoreML. Check each model's licence first.
Protocol: from the replayed footage, take every pair of crops from the same physical item (positives) and from different items (negatives), including the
same-colour pairs. Report ROC AUC, the threshold that gives at most 1% false merges and what false-split rate that costs, vector size, ms per crop.
Note the open question in `VisionFeaturePrintEmbedder.swift` (cosine on raw feature prints is a hypothesis).
Output: `docs/results/phase1-embedder.md` plus a recommended default and `sku_similarity` value.

### 1.9 Clustering upgrades (PROPOSE FIRST: these touch the state machine / contracts)
PLAN.md section 5 steps 2 and 4: a category gate (different top-level categories never merge), keypoint / OCR verification in the ambiguous band, and the
"same product?" question to the user with both crops. Each needs a design note in `docs/results/` and Matt's approval before code: they add an event
(for example `Event::SameProductQuestion`) and probably a plugin trait (`PairVerifier`), which CLAUDE.md says to ask about.

### 1.10 Barcode bake-off and the flip-for-barcode flow
Compare `RxingScanner` and `VisionBarcodeScanner` on the same frames (decode rate, time to a 2-frame agreement, false reads). Exercise focus mode:
`focus_item`, `BarcodeRequested`, the hi-res barcode side path. Decide the default per platform.

### 1.11 Cloud quality and privacy audit (needs an Anthropic key; Matt decides whether to run it)
Run `--anthropic` on the 10 items in both modes. Compare titles and descriptions with the truth sheet; check that unknown details are marked
"Needs your input" instead of invented. **Privacy audit:** dump the exact request bodies (add an env switch such as `SHELFSCAN_DUMP_REQUESTS=dir`, key excluded)
and confirm only masked product crops leave the machine, never a full frame (CLAUDE.md invariant 2).

### 1.12 CSV import
`products.csv` from the 10-item scan into a development store (Shopify admin -> Products -> Import). Matt does the click; record exactly what the importer
reports. Fix the column set if the importer complains (headers were taken from Shopify's help pages and never import-tested).

## Acceptance tests (proposed numbers; Matt adjusts them)

On session A (good light), live pipeline with the recommended plugins:
1. 10 SKUs found; **0 false merges**; at most 1 false split.
2. Quantity exact for at least 9 of 10 items.
3. Every item reaches `Confirmed` through the real review flow; items with barcodes reach `Confirmed(Barcode)`.
4. Sessions B and C: at most 1 false merge and at most 2 false splits each.
5. Live preview: p95 glass-to-box latency at most 500 ms on the Mac; dropped-frame counter visible; no UI hitch while scanning.
6. `products.csv` imports into the dev store without errors; all products are drafts.
7. `cargo test --workspace --all-features`, clippy `-D warnings`, `scripts/python_smoke.sh` still pass; PLAN.md section 10 updated.

## Out of scope for Phase 1
iPhone UI, Keychain, live Shopify API calls, images in listings, Android, aisle walk-through.
