# Progress log (Phases 1 to 3)

Running log. Newest entry at the bottom. If context resets: read this file, then the phase doc for the current branch.
Format per entry: date, item, did, measured, blocked, next.

## 2026-10-25 · 1.1 Baseline on the Mac

* **Machine:** Apple M3 Pro, macOS 26.3.1 (25D2128), Xcode 26.0.1 (17A400), Swift 6.2, rustc 1.91.1, Homebrew ffmpeg, python3.
* **Did:** `git init`, "Phase 0 snapshot" commit on `main`; branch `phase-1`.
* **Measured:**
  * `cargo test --workspace --all-features`: 54 unit + 24 pipeline pass. Pipeline suite run 4 times, 4 of 4 green (~5.4 s each, vs ~22 s in the sandbox).
  * `cargo clippy --workspace --all-targets --all-features -- -D warnings`: clean.
  * `scripts/python_smoke.sh`: passes.
  * CLI synthetic demo: 3 products, 3 draft rows in `out/products.csv`.
  * `scripts/build_xcframework.sh`: **passes on first run** (aarch64-apple-ios, -ios-sim, -darwin; ~48 s release build each).
  * `cd apple && swift build`: failed first. Cause: built-in plugin impls named parameters differently from the trait (`req`, `g`), and UniFFI turns those into Swift argument labels, so `PlaceholderGuesser`, `AnthropicGuesser`, `OpenFoodFactsLookup`, `StubPriceAdvisor` did not conform to their Swift protocols. Fixed by renaming impl parameters (trait unchanged). After the fix `swift build` passes with 0 errors and 0 warnings, including the Vision and Capture sketches (Swift 5 language mode, tools 5.9).
* **Blocked:** dataset (1.5), Anthropic key permission (1.11), iPhone + team (Phase 2), Shopify dev store + credentials (3.0), CSV import click (1.12). Asked Matt in one message.
* **Next:** 1.2 fixtures subcommand + XCTest.

## 2026-10-25 · 1.2 Swift package

* **Did:** `shelfscan fixtures --out DIR` (scene PNGs, EAN-13 card, 12 barcode-on-table variants, `fixtures.json`; Rust test included);
  `apple/Tests/ShelfScanTests` (EngineTests: Swift-built Engine + Swift listener, waits for `ScanFlushed`); `scripts/swift_test.sh`.
  Fixtures are generated and git-ignored (3.1 MB of noisy PNGs).
* **Measured:** `swift build` + `swift test` pass on macOS; 3 of 3 synthetic products discovered from Swift.
  Swift note: `EngineConfig(defaultMode:)` is the only field without a Swift default (enum defaults are not generated).
* **Blocked:** iOS build check: the iOS 26 platform is not installed in Xcode (`xcodebuild -downloadPlatform iOS`, several GB). Needed by Phase 2.
* **Next:** 1.3.

## 2026-10-25 · 1.3 Vision plugins (synthetic)

* **Did:** `VisionTests.swift`: barcode decode table (Vision vs rxing), feature-print shape / norm / self-cosine, foreground boxes vs truth, mask origin, padded BGRA path.
* **Measured:** see `docs/results/phase1-vision-unit.md`. Vision 8/12 vs rxing 7/12 barcode variants, no false reads; Vision first call ~900 ms then ~6 ms.
  Feature print 768-d, same/different synthetic product cosine 0.949 / 0.858. Foreground IoU ≥ 0.946, mask top-left origin confirmed.
* **Blocked:** "5 real photos of barcodes" and detector on real frames need real items (1.5).
* **Next:** 1.6 (replay + eval), then 1.4 (Mac demo app).

## 2026-10-25 · 1.6 Deterministic replay + scoring

* **Did:** `--offline` for ffmpeg file sources (ffmpeg resamples to `analysis_fps`, timestamps from the frame index, wait for a credit instead of dropping;
  novelty / blur gates stay as live). `shelfscan score --run DIR --truth truth.json [--labels]` and `--truth` on a scan: labels tie items to truth
  names (auto from GTIN; otherwise asked on the terminal with the crop path, saved to `labels.json`); reports SKUs found, false merges (and same-colour merges),
  false splits, not-products, truth items only hidden, quantity exact; writes `eval.json`. The scorer reads a minimal item shape so a Swift replay tool can feed it.
* **Measured** (synthetic 10 s clip made from the fixture scene, 480x360): offline 60 frames in, 0 dropped, 17 novelty skips, 43 processed, 3/3 SKUs,
  **two runs identical**; live `-re` replay of the same clip processed 33. Score with labels: 3/3 found, 0 merges, 0 splits, quantity 3/3.
  Non-interactive scoring without labels fails with the list of unlabelled items (by design). Interactive labelling: not exercised (needs a TTY).
* **Blocked:** real footage (1.5).
* **Next:** 1.4 Mac demo app.

## 2026-10-25 · 1.4 Mac demo app

* **Did:** `apple/Examples/MacDemo` (SwiftPM executable; NSCameraUsageDescription embedded). Preview + overlay, item list with
  confirm / reject / title / hide / focus, draft listings, CSV export (~/Documents/shelfscan/products.csv), stats strip
  (frames, boxes, hidden, latency, hitches). Matt asked mid-way for the segmenter to be switchable and parameters tunable in the UI:
  added a Settings tab (detector Blob / Vision, masks on/off, Vision min instance size, Blob work size + threshold, embedder,
  barcode scanner, every EngineConfig knob), applied by rebuilding the engine, saved in UserDefaults. New Rust constructor
  `BlobDetector::with_params` (mentioned to Matt). `--measure N` and `--overlay-test --selftest` modes.
* **Measured:** `docs/results/phase1-live-latency.md`. p95 82 ms (Blob) to 122 ms (Vision + masks), 0 hitches, 0 drops.
* **Bugs found live:** (1) overlay mirrored vertically (Matt's screenshot), fixed and covered by the self-test; (2) measure mode
  hung when SwiftUI created no window at launch; launch logic moved to the app delegate.
* **Open:** Vision foreground calls a person a product. Option: a Swift detector wrapper that labels people (Vision human
  rectangles) so Track gives them the Background role; would need agreement since roles are the engine's call. Not done.
  Real-object box check on screen still to do with the 1.5 items.
* **Next:** remaining unblocked work: 1.9 design note (PROPOSE FIRST), 1.11 request-dump switch (code only, no live call).

## 2026-10-25 · Detector / embedder / describer options (Matt's requests during 1.4)

* **Did:** CoreML DETR semantic-segmentation detector (Apple's conversion); SAM 2.1 tiny mask refinement over any detector's
  boxes; MobileCLIP S0/S2 embedder and zero-shot describer (research-only licence, Matt's choice); CLIP tokenizer port;
  HybridGuesser (on-device gate + hints, Claude names products); 1.11 request dump switch; all selectable in the demo's Settings.
  Placeholder guesser now uses a specific detector class as its noun.
* **Measured:** `phase1-foundation-models.md`, `phase1-live-latency.md`. DETR 29.8 ms (ANE); SAM encoder 120 ms (ANE) + 7 ms/box (GPU);
  MobileCLIP S2 6.8 ms. Full stack live p95 1246 ms with 4 frames in flight → 417 ms with 1.
* **Decisions by Matt:** MobileCLIP despite research-only weights (recorded; SigLIP 2 is the replacement); SAM 2.1 tiny yes;
  naming = CLIP local + Claude; may use his Anthropic key (masked crops only, dumps for audit).
* **Blocked:** ANTHROPIC_API_KEY is not in the environment Claude Code's shell sees. Matt to put it in
  `~/.config/shelfscan/anthropic_key` (chmod 600) or start the demo from a shell that exports it.
* **Open:** ~450 ms UI hitch with the CLIP/SAM stack; engine build on the main thread at Apply; over-splitting of one person.
* **Next:** first live Claude call through the demo with request dumps, audit the dumps, then 1.9 design note.

## 2026-10-25 · Phase 1 closed (Matt's call) with open items

* **Did:** barcode flow verified through the engine with camera-format frames (Vision + rxing); fixed Vision barcode cold start
  (warm-up) and slow RGB→RGBA conversion (vImage); overlay Boxes / Labels toggles (self-tested); guided barcode UI (Scan barcode,
  typed barcode, auto-unfocus on read); `phase1-report.md`; PLAN §10 and known-issues updated.
* **Measured:** Rust 57 + 24 (3/3 runs) + 5 CLI; clippy clean; Python smoke OK; Swift 23 tests debug + release; self-test OK.
* **Open (carried):** dataset and bake-offs (1.5, 1.7, 1.8), 1.9, CSV import (1.12), live webcam barcode check, ~450 ms hitch,
  over-splitting, cloud response parsing not confirmed.
* **Next:** Phase 2 needs the iPhone details and the iOS platform download; Phase 3.0 needs the dev store and client id / secret.
