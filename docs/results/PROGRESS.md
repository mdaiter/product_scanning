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
