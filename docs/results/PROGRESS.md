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
