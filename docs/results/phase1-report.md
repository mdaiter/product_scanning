# Phase 1 report (Mac) · 2026-10-25

Machine: Apple M3 Pro, macOS 26.3.1, Xcode 26.0.1, Swift 6.2, Rust 1.91.1. Branch history is on `main` (`origin`).
Phase 1 is **closed by Matt's decision** with the items below still open: the tabletop dataset (1.5) was never recorded and no
Shopify dev store exists yet, so the acceptance tests that need them were **not run**. Nothing below is marked passed that did not run.

## Acceptance tests (`docs/phase-1-mac.md`)

| # | Test | Result | Evidence |
|---|---|---|---|
| 1 | Session A: 10 SKUs, 0 false merges, ≤ 1 false split | **NOT RUN** | No 10-item dataset (1.5). The scorer exists (`shelfscan score`, 1.6) and was checked on a synthetic clip: 3/3, 0 merges, 0 splits. |
| 2 | Quantity exact for ≥ 9 of 10 items | **NOT RUN** | Same. |
| 3 | Every item reaches `Confirmed` through the review flow; barcode items reach `Confirmed(Barcode)` | **PARTIAL** | Engine-level, with camera-format frames (padded BGRA 1280×720): discover → focus → show EAN-13 → validated GTIN after 2 agreeing reads → `Confirmed(barcode)`, with **both** Vision and rxing (`BarcodeFlowTests`, debug and release). Review actions (✓, ✗ / re-guess, typed title, hide, scan barcode, typed barcode) are in the Mac demo. **Not yet done:** a live barcode shown to the webcam, and the 10 real items. |
| 4 | Sessions B, C: ≤ 1 false merge, ≤ 2 false splits each | **NOT RUN** | No dataset. |
| 5 | Live preview: p95 glass-to-box ≤ 500 ms, dropped counter visible, no UI hitch | **PASS with one caveat** | p95 82 ms (Blob), 113–122 ms (Vision), 136–145 ms (DETR), 417 / 436 ms (DETR + SAM + MobileCLIP, 1 frame in flight; 960 / 1920 px). Counters on screen. 0 hitches in most runs; **a ~450 ms main-thread hitch appeared in some runs of the SAM + MobileCLIP stack and is not diagnosed**. Room scene, not the test table. `phase1-live-latency.md`, `phase1-foundation-models.md`. |
| 6 | `products.csv` imports into the dev store without errors; all drafts | **NOT RUN** | No dev store yet (needs Matt, 1.12 / 3.0). CSV parse-back test passes. |
| 7 | Tests, clippy `-D warnings`, Python smoke pass; PLAN §10 updated | **PASS** | Rust 57 unit + 24 pipeline (3 of 3 runs), 5 CLI; clippy clean; Python smoke OK; Swift 23 tests (debug and release) + overlay self-test. PLAN §10 updated with this report. |

## Work items

| Item | Status |
|---|---|
| 1.1 Baseline on the Mac | done (`PROGRESS.md`) |
| 1.2 Swift package, XCTest, fixtures | done; iOS build not tried (iOS platform not installed) |
| 1.3 Vision plugins | done on synthetic fixtures; real photos not done |
| 1.4 Live Mac demo | done: preview, overlay (toggles for boxes and labels), item list and review actions, barcode flow, settings for every plugin and `EngineConfig` knob, stats, latency, CSV export |
| 1.5 Dataset | **not done** (needs items and sessions) |
| 1.6 Offline replay + scorer | done |
| 1.7 Detector bake-off | **not done on footage**; candidates built and timed: Blob, Vision foreground, CoreML DETR (Apple), SAM 2.1 tiny refinement |
| 1.8 Embedder bake-off | **not done on footage**; candidates built and timed: Color, Vision feature print, MobileCLIP S0/S2 (research-only licence). `sku_similarity` is still the placeholder 0.92 |
| 1.9 Clustering upgrades | **not started** (propose-first) |
| 1.10 Barcode bake-off | synthetic decode table (Vision 8/12, rxing 7/12, no false reads) + engine flow with both; on-camera comparison not done. Default for the Mac demo: Vision (warm-up fixed its cold start) |
| 1.11 Cloud quality and privacy audit | request dump switch built and tested; live: 95 requests audited by count and size, 6 crops inspected by eye (masked product crops, no faces seen), key in 0 files. Quality vs a truth sheet not done (no dataset) |
| 1.12 CSV import | **not done** (needs a dev store) |

## Added beyond the spec (Matt's requests during the phase)

CoreML DETR detector; SAM 2.1 tiny pixel masks at frame resolution; MobileCLIP embedder and zero-shot describer; CLIP
tokenizer port; hybrid naming (on-device gate keeps people out of cloud requests, Claude names products, on-device words
as hints only when confident, tiny crops not sent); pinned and checksummed model downloads.

## Known open problems carried forward

* ~450 ms UI hitch with the SAM + MobileCLIP stack (intermittent).
* One object becomes several items (placeholder `sku_similarity`; needs the 1.8 calibration on real footage).
* Barcodes are read only while an item is focused (by design); the demo guides the user, but there is no "scan any barcode" mode.
* A better view later does not re-ask the cloud for a name (would touch the state machine: ask first).
* MobileCLIP weights are research-only; SigLIP 2 (Apache-2.0) is the replacement before any commercial use.
