# shelfscan: plan of record

Scan a store with a phone camera, index distinct products, draft one Shopify listing per product,
let the user confirm or correct what the system guessed. Rust core, UniFFI bindings, `ractor`
actors on tokio. iPhone first.

This file is the spec. `CLAUDE.md` holds the rules for working in the repo. Anything marked
**UNVERIFIED** has not been run or checked against the real service/device; treat it as a
hypothesis and test it first.

## 1. Decisions (from the planning conversation)

| # | Decision | Notes |
|---|---|---|
| 1 | Actors: `ractor` on `tokio` | Replaces the hand-rolled actor runtime. |
| 2 | Platform order: iPhone first, Android later | Apple plugins (Vision, CoreML) are Swift; the Rust core stays portable. |
| 3 | v1 acceptance scenario: **tabletop** capture | Items shown one at a time or a few at once. Aisle walk-through is a stretch goal (section 8). |
| 4 | Two identity strategies behind a trait | `BarcodeFirst` (catalog mode) and `DescribeFirst` (handmade/unbranded). Session default, per-item override. |
| 5 | Per-item state machine owned by `Catalog` | Unknown, Guessed, Confirmed(user or barcode), Rejected. |
| 6 | Roles instead of drop filters | Every detection is Product, Background (auto) or Excluded (user). Nothing is silently discarded. |
| 7 | One listing per product | Variant grouping is a later feature. |
| 8 | Listings are always `DRAFT` | Never `ACTIVE` from this library. |
| 9 | Storefronts: Shopify CSV and Shopify Admin API | CSV image URLs are deferred. |
| 10 | SKU policy: generate `PREFIX-seq` | Editable. Registry in the Store. GTIN goes in the barcode field when known. |
| 11 | Cloud allowed but pluggable and opt-in | Reference impl: Anthropic API (Claude vision). Masked product crops only, never full frames. |
| 12 | Price data: `PriceAdvisor` trait, **stub only** in v1 | User-click triggered, cached. No live source wired in. |
| 13 | Store: trait, in-memory default, SQLite impl | SQLite impl does keyword (FTS5) + vector + fused ranking. |
| 14 | Copy style: per-store style prompt, English | |
| 15 | Quantity is an estimate and is **not** pushed to Shopify inventory by default | |
| 16 | Shopify credentials: kept in the iOS Keychain; a shipped phone app talks to a backend proxy | **Revised 2026-10-24.** Shopify stopped allowing new legacy custom apps on 2026-01-01, so "owner pastes an Admin token" no longer works for new setups. The library supports a static token (existing legacy apps) and the Dev Dashboard `client_credentials` grant (24 h tokens, same-organization stores only). Other merchants' stores need custom distribution plus OAuth in a backend. See `docs/phase-3-shopify-live.md`. |
| 17 | Variant barcodes are sent as `barcodes: [{ value }]` | `ProductVariantSetInput.barcode` is deprecated (shopify.dev, 2026-10). |
| 18 | UI gets results through events, not return values | `docs/events.md` is the contract (async, ordered, `Overlay`/`Stats` lossy). |

## 2. Architecture

```
NATIVE APP (Swift / Kotlin / Python / Rust)
+--------------------------------------------------------------------------+
| camera -> Frame (NV12/BGRA)    review UI (overlay, confirm/reject)       |
| native plugins: Detector, Embedder, BarcodeScanner, ProductGuesser, ...  |
+--------+-----------------------------^------------------------^----------+
         | push_frame / wants_frame    | EventListener          | impl traits (UniFFI with_foreign)
=========|====== UniFFI boundary =======|========================|==========
         v                              |                        |
+--------------------------------------------------------------------------+
| Engine (UniFFI object): owns tokio runtime, credit counter, ActorRefs    |
+--------+-----------------------------------------------------------------+
         v
  [Supervisor] --spawn_linked--> every actor below
```

Pipeline:

```
frame -> push_frame(): credit gate. No credit -> DROP at the edge, return false
   v
[Ingest]  convert to RGB, downscale to <=960px; novelty + blur gates; keeps a hi-res copy
   |      for [Barcode] only while a focus item is set
   v
[Detect]  Detector plugin: boxes (+masks), score/area filters, NMS     (blocking pool)
   v
[Track]   association (IoU + colour signature), role heuristics, best crops;
   |      focus mode: binds the best central detection to the focused item
   |      promoted at N hits -> early guess + overlay
   v
[Embed]   Embedder plugin -> appearance vector                         (blocking pool)
   v
[Catalog] OWNS item state. Clusters tracks into SKUs, fixture-library check, state machine
   |  new SKU -> [Guess] (ProductGuesser plugin) -> back to Catalog
   |  gtin read -> [Lookup] (ProductLookup plugin) -> back to Catalog
   v
[Overlay] merges per-frame boxes (Track) with item state (Catalog) -> Event::Overlay

on demand (user clicks):
 generate_listings() -> [Lists]   ListingWriter plugin -> drafts (kept for edits)
 publish(ids, name)  -> [Publish] Storefront plugin     -> receipts (chunked)
 suggest_price(id)   -> [Pricing] PriceAdvisor plugin   -> cached comps

side channel (lossy, never blocks): every actor -> [Events] -> EventListener (foreign callback)
```

Rules the code must keep:

* **One owner per piece of state.** `Catalog` is the only writer of item state. UI commands and
  plugin results arrive as messages.
* **Backpressure at the edge.** `ractor` mailboxes are unbounded (observed in 0.16.x, not just assumed).
  So `push_frame` takes a credit; the credit is released when the frame leaves the pipeline. No credit
  means the frame is dropped and counted. Tested in `tests/pipeline.rs` (credit accounting, drops).
* **All plugin traits are synchronous.** The engine calls them from `spawn_blocking`, so a slow
  model or HTTP call never stalls actor scheduling.
* **Flush barrier.** `flush()` sends a token through the same mailboxes as the data. FIFO order
  gives "everything pushed before this has been processed". `Catalog` holds the token until
  outstanding guesses finish.
* **Keep `panic = "unwind"`.** `ractor` reports a panicking actor to its supervisor; with
  `panic = "abort"` the process dies. A dead actor means `Event::Error` and the engine can be rebuilt
  from the Store.

## 3. Item state machine

```
 UNKNOWN --guess ready--> GUESSED --user checkmark--------> CONFIRMED(user)
    ^                       |  \
    |                       |   \--valid barcode read-----> CONFIRMED(barcode) --lookup--> enriched
    |                       |
    |                       +--user X--> REJECTED --user types a title--> CONFIRMED(user)
    |                                        |
    +------ auto re-guess (max 2, ---------- +
            excluding rejected titles)

 A barcode arriving in UNKNOWN or REJECTED also goes to CONFIRMED(barcode).
 CONFIRMED(user) upgrades to CONFIRMED(barcode) if a barcode shows up later.
 needs_barcode stays true until a barcode is read or the user skips it.
```

* A barcode is accepted only if the GTIN check digit validates and **two frames agree**.
* Confirmation is per SKU: one checkmark covers every unit in that cluster.
* A typed correction on reject counts as a user confirmation.
* Orthogonal to state: `role` (Product / Background / Excluded) and `mode` (BarcodeFirst /
  DescribeFirst).

### Identity strategies

```rust
trait IdentityStrategy: Send + Sync {
    fn needs_barcode(&self, item: &ItemView) -> bool;
    fn auto_confirm(&self, item: &ItemView, evt: &IdentityEvent) -> Option<ConfirmedBy>;
    fn wants_rich_description(&self, item: &ItemView) -> bool;
}
```

* `BarcodeFirst`: flag every non-barcode item for a flip-and-scan, capture several views, look up
  the GTIN, treat the barcode as ground truth.
* `DescribeFirst`: never nag for barcodes; generate title, SEO description and tags from the best
  views via the `ListingWriter`; the user approves the text.

## 4. Roles and noise control

Every detection gets a role. Defaults are heuristics; the user can override.

* **Background (auto):** box area above `background_area` (default 0.5), matches a fixture-library
  vector, or sits off-centre and soft while a focus item is set.
* **Excluded (user):** per box, per SKU ("exclude all like this" stores the vector in the fixture
  library so the same decor is hidden across the session and across sessions), or by painted region.
* The UI shows likely products plus a "N hidden" chip. Hidden items are never deleted.

## 5. Extension points (traits)

All are `#[uniffi::export(with_foreign)]` and also implementable in Rust.

| Trait | Input -> output | Built-in default | Planned plugins |
|---|---|---|---|
| `Detector` | RGB frame -> boxes (+masks) | classical blob fallback (demo/CI only) | Vision foreground-instance masks, RT-DETR/RF-DETR via CoreML or LiteRT, `tract` DETR |
| `Embedder` | RGB crop -> vector | colour histogram (weak) | Vision feature print, small CLIP/SigLIP-class CoreML, DINOv2-small |
| `BarcodeScanner` | hi-res frame -> reads | `rxing` (ZXing port), feature `barcode` | Apple Vision, ML Kit |
| `ProductGuesser` | crop + hints -> title/brand/category | placeholder | Anthropic API (`cloud::anthropic`, feature `net`, built, mock-tested), on-device VLM |
| `ProductLookup` | GTIN -> product data | none | Open Food Facts (`cloud::off`, built, mock-tested), Verified by GS1 (owner), commercial |
| `ListingWriter` | catalog item -> listing | template with placeholder text | Anthropic API (built, mock-tested; SEO title/description from a per-store style prompt) |
| `PriceAdvisor` | item -> price range + comps | stub that errors | third-party sold-comps, eBay Browse (active), Google Merchant benchmark |
| `Storefront` | listings -> receipts | `CsvStorefront`, `ShopifyStorefront` (DRAFT; built, mock-tested) | anything |
| `EventListener` | events | none | UI |

Tie-break for SKU merging (the embedder alone is not trusted):

1. Same GTIN means same SKU; different GTIN means different SKU, regardless of embedding.
2. Category gate: different top-level categories never merge.
3. Similarity bands: at or above `hi` merge, at or below `lo` split, between is ambiguous.
4. Ambiguous: verify with local keypoint matching and OCR text overlap (flavour text differs).
5. Still ambiguous: ask the user "same product?" with both crops side by side; log the labelled pair
   to calibrate `hi`/`lo` per embedder.

Phase 0 implements 1, 3, and the user question hook. Steps 2 and 4 are Phase 1.

## 6. Store

`Store` trait: items, fixture library, SKU registry, price cache, keyword + vector + fused search.

* In-memory implementation: default.
* SQLite implementation (`rusqlite`, bundled): FTS5 keyword search, vectors stored as blobs, cosine
  computed in Rust, results fused with reciprocal rank fusion. Fine for thousands of items.
* `sqlite-vec` is an optional accelerator. **Probe result (Linux x86_64 only):** FTS5 BM25, cosine
  KNN through `vec0`, and RRF fusion all ran in one bundled SQLite connection. `0.1.10-alpha.4`
  fails to compile (the crate is missing `sqlite-vec-diskann.c`); `=0.1.9` works. It is pre-v1 and
  its README does not mention iOS/Android: **UNVERIFIED on phones**.

## 7. Price data (access reality)

Sources: third-party article for eBay, Google's own docs for the benchmark report.

* eBay: reported that `findCompletedItems` / Finding API was shut down in Feb 2025, the Marketplace
  Insights API (sold data) is limited-release for approved partners, and the sold-items page filter
  needs a login. **Not checked against eBay's docs.**
* Google: the price-competitiveness report benchmarks products you already sell in your own
  Merchant Center account. Docs do not describe arbitrary lookups.
* Therefore v1 ships the trait, the click-to-request UI hook, caching keyed by GTIN/title with a
  TTL, and a stub. A source is wired in only when you have access to one.

## 8. Capture profiles

* **Tabletop (v1):** one to a few items, focus mode, multi-angle capture, barcode flip.
* **Aisle walk-through (stretch):** evidence from the sample video (9.5 s handheld Halloween-store
  reel, 55 frames analysed at 6 fps):
  * median camera shift between analysed frames 6.6% of width, p90 14.9%. IoU-only tracking would
    drop product-sized boxes, so Track needs camera-motion compensation (ORB + RANSAC gave median
    161 inliers per pair) or a motion hint from the phone.
  * 5 of 54 pairs lost overlap (two edit jump cuts and one whip pan); 5 of 55 frames were below
    half the median sharpness.
  * the last ~1.2 s were identical frames; the source is variable frame rate (about 35 fps average
    though labelled 60), so use timestamps, not frame counts.
  * the scene mixes merchandise with non-merchandise (animatronics, statue, mannequin, fixtures) and
    has very large objects, which is why roles replace drop filters.
  * no detector was run on it, so detection noise there is unmeasured.

## 9. Phases and acceptance

Detailed specs with work items and acceptance tests are in `docs/`. Hand-off entry point: `HANDOFF.md`.

**Phase 0 (portable core): DONE in the Linux sandbox**
* `ractor` pipeline with supervision and restore, explicit backpressure, FIFO flush barrier, plugin panic isolation.
* State machine, roles, identity strategies, Store (memory + SQLite with FTS5, vectors and fused ranking), GTIN validation.
* Storefronts: Shopify CSV and Shopify Admin API (DRAFT; static token or client credentials).
* Cloud reference plugins: Anthropic guesser / writer, Open Food Facts lookup (all mock-tested).
* UniFFI surface; Python bindings exercised with Python-implemented plugins; Swift bindings generated (not compiled).
* CLI demo (synthetic, image folder, ffmpeg pipe) with a review loop.
* 54 unit + 24 pipeline tests, clippy clean.

**Phase 1 (Mac): CLOSED 2026-10-25 with open items** (`docs/results/phase1-report.md`: tests 1, 2, 4, 6 not run for lack of the dataset and dev store; 3 partial; 5 and 7 pass). `docs/phase-1-mac.md`. Vision plugins, live preview app, dataset, detector and embedder bake-offs, calibrated clustering, CSV import.
**Phase 2 (iPhone app shell)**: `docs/phase-2-iphone.md`. Capture, review UI, lifecycle, Keychain, backend-proxied cloud, on-device measurements.
**Phase 3 (live Shopify)**: `docs/phase-3-shopify-live.md`. First live drafts, images, idempotency, throughput, backend design.

## 10. Verified vs unverified

"Ran" means executed in this project. "Docs" means read on the vendor's own documentation (date given) but not exercised. Everything else is UNVERIFIED.

| Item | Status |
|---|---|
| Rust 1.97 builds the workspace; crate versions in `Cargo.toml` | ran (Linux x86_64); also Rust 1.91.1 on macOS 26.3.1 / M3 Pro (2026-10-25) |
| Actor pipeline, state machine, roles, strategies, Store, publish chunking, flush barrier, credits, supervisor restarts | ran: 24 pipeline tests, 8 of 8 consecutive green runs (Linux); 4 of 4 on the Mac (2026-10-25) |
| `ractor` mailboxes unbounded | observed (0.16.x) |
| FTS5 + vector + RRF on bundled SQLite | ran (Store contract tests on memory and SQLite) |
| `sqlite-vec` 0.1.9 | ran on Linux x86_64 only in an early probe; **not used by the code** (cosine in Rust) |
| UniFFI Python bindings: foreign plugins, typed errors, async, built-ins passed from Python | ran (`scripts/python_smoke.sh`, Linux and Mac) |
| UniFFI Swift bindings generation | ran |
| `scripts/build_xcframework.sh` (aarch64-apple-ios, -ios-sim, -darwin) | ran (Mac, Xcode 26.0.1, 2026-10-25) |
| `swift build` of the package (ShelfScan, ShelfScanVision, ShelfScanCapture) for macOS | ran (Mac, Swift 6.2 in Swift 5 mode) after renaming impl parameters to match trait labels |
| `rxing` EAN-13/8, UPC-A/E decode of our own rendered barcodes | ran (encoder/decoder cross-check; real focused-item barcode confirm test) |
| `rxing` quality on webcam / phone frames | **UNVERIFIED** |
| CLI: synthetic, image folder, review loop | ran |
| CLI: ffmpeg pipe source reading a video **file** (Linux, with `-re`) | ran (found the one object in a generated clip) |
| CLI: ffmpeg pipe source from a Mac webcam (`avfoundation`) | **UNVERIFIED** (the Mac demo uses AVFoundation directly instead) |
| CLI `--offline` replay of a video file, `shelfscan score` | ran (synthetic clip, two runs identical) |
| Shopify CSV headers | docs (help.shopify.com, 2026-10); parse-back test ran; **importer UNVERIFIED** |
| Shopify `productSet` field names (`status DRAFT`, `sku`, `barcodes`, `seo`, `productOptions` max 3, `variants`, `files`, `inventoryQuantities`, `userErrors`) | docs (shopify.dev API 2026-10, 2026-10-24); request shape mock-tested; **live store UNVERIFIED** |
| Shopify client-credentials grant (endpoint, form body, 24 h token, same-org limit) | docs (2026-10-24); mock-tested; **live UNVERIFIED** |
| Shopify: `productSet` upsert via `identifier`; staged upload to `productSet.files`; image upload; inventory | **UNVERIFIED** (docs partial, see Phase 3) |
| Shopify rate limits at 100 listings | **UNVERIFIED** |
| Anthropic Messages API request shape and forced tool call | **requests ran live** from the Mac demo (2026-10-25, 95 calls to `/v1/messages`, model `claude-sonnet-5-5`, masked crops only, audited from `SHELFSCAN_DUMP_REQUESTS` dumps, key in 0 files); **response parsing not confirmed** (responses are not dumped; Matt has not reported the titles yet) |
| Open Food Facts response shape | mock only; **live UNVERIFIED** |
| Video measurements in section 8 | measured earlier (ORB / Laplacian) on one reel, no detector run |
| Rust test suite, clippy, Python smoke on a Mac | ran (macOS 26.3.1, M3 Pro, 2026-10-25) |
| Anything on an iPhone or Android | **UNTESTED** |
| Vision foreground-instance masks on dense racks | **UNTESTED** (Apple's page gives no instance limit) |
| Swift ↔ Rust round trip: Engine built in Swift, Swift `EventListener`, `ItemDiscovered` for 3 synthetic products | ran (`swift test`, macOS, 2026-10-25) |
| Vision plugins (barcode, feature print, foreground masks) on **synthetic** fixtures: EAN-13 decode, 768-d unit vectors, mask top-left origin, padded BGRA frames | ran (`swift test`, macOS; `docs/results/phase1-vision-unit.md`) |
| Vision plugins on webcam frames | foreground detector and feature print ran live (room scene, 2026-10-25); quality against a truth sheet **UNTESTED** (needs the 1.5 items); Vision barcode on a real webcam **UNTESTED** |
| `ShelfScanCapture` (CameraSource, ScanModel, LatencyMeter) | ran live in the Mac demo (FaceTime HD Camera, 1920x1080, host-clock timestamps); the SwiftUI `OverlayView` is unused (the demo draws with layers) |
| Mac demo overlay geometry and Boxes / Labels toggles | ran (off-screen self-test in `scripts/swift_test.sh`; caught a vertically mirrored overlay on the first live run) |
| Flip-for-barcode flow through the engine (focus → 2 agreeing reads → `Confirmed(barcode)`), camera-format frames | ran (`BarcodeFlowTests`, Vision and rxing, debug and release) |
| CoreML DETR (Apple, Apache-2.0), SAM 2.1 tiny (Apple, Apache-2.0), MobileCLIP S0/S2 (Apple, **research-only licence**) | ran (unit tests + live timing, `docs/results/phase1-foundation-models.md`) |
| Detector / embedder bake-off on tabletop footage, calibrated `sku_similarity` | **NOT RUN** (no dataset) |
| Swift package for iOS | **UNTESTED** (iOS platform not installed in Xcode on the dev Mac) |
| eBay / Google price-access claims | secondary sources, not the vendors' docs |
| Open Food Facts / GS1 terms | read from their pages; legal review is yours |

## 11. Risks

* Weak default embedder will merge same-coloured different products; one checkmark then confirms
  both. Mitigation: real embedder in Phase 1, barcode split, "same product?" prompt.
* Generated text invents details (material, size, condition). Writers must mark those fields
  "needs your input" instead of guessing.
* Privacy: store video can contain people. Only masked crops go to a cloud model, and only when
  the app opted in.
* Open Food Facts data is under ODbL (attribution and share-alike); GS1's free tier is limited to
  30 queries per day.
* An Admin API token inside a shipped phone app is a security problem. Use a backend proxy.
