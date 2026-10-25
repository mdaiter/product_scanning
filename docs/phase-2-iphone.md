# Phase 2: iPhone app shell

**Goal.** The tabletop flow on a real iPhone: scan, review, edit drafts, export CSV (and call Shopify once Phase 3 lands),
using the Phase 1 plugins. This phase is where on-device reality (heat, memory, orientation, lifecycle, permissions) shows up.

**Gate in.** Phase 1 acceptance passed and Matt reviewed it.

**Gate out.** Acceptance tests below, then PLAN.md section 10 updated and a stop for review.

## Prerequisites

* A physical iPhone (Matt to say which model and iOS; the manifest assumes iOS 17+) and an Apple Developer team for signing. Simulator has no camera.
* Decisions from Matt (HANDOFF.md, "Questions"): bundle id, team, where a backend proxy would live.

## Work items

### 2.1 App target
`apple/ShelfScanApp` (SwiftUI, iOS 17). Depends on the `ShelfScan`, `ShelfScanVision`, `ShelfScanCapture` products. Info.plist: `NSCameraUsageDescription`.
If you generate the Xcode project (XcodeGen or similar), commit its spec, not the generated project.

### 2.2 Camera on a phone
* Portrait. Rotate the frame **before** `push_frame` (iOS 17: `AVCaptureConnection.videoRotationAngle`), so engine boxes are in portrait space (`docs/events.md`, "Coordinates").
* Overlay mapping for aspect-fill if you want a full-bleed preview.
* Tap to focus and expose. **Colour stability:** the default embedder is colour-driven; auto exposure / white balance drift changes colours between frames.
  Offer lock-AE/AWB during a scan and measure its effect on false splits.
* Torch toggle for dim rooms.

### 2.3 Lifecycle and resources
* Scene phase `.background`: stop the session, `flush()`. Back in the foreground: `resume()` (the SQLite store restores the session).
* Interruptions (phone call, another camera app): handle `AVCaptureSession` interruption notifications.
* Thermal: observe `ProcessInfo.thermalState`; at `.serious` lower `analysis_fps` (rebuild config) or pause with a message.
* Memory: `max_images_per_item` and JPEG sizes bound the per-item footprint; respond to memory warnings. Record peak memory over a 5-minute session.

### 2.4 Screens
* **Scan:** preview + overlay, "N hidden" chip, tap a box to focus it, barcode-flip prompt from `BarcodeRequested`, mode toggle (session default, per-item override).
* **Review queue:** one card per item: crop, guess, confidence, quantity. Actions: confirm, reject (re-guess), type the title, hide ("exclude all like this" stores a fixture), enter barcode manually.
* **Listing editor:** title, description (HTML preview), price, tags, vendor, product type, SEO fields; fields the writer could not know are marked "needs your input" and must be visibly unresolved. Saves with `update_listing`.
* **Publish:** destination (CSV via the share sheet, Shopify), per-listing receipts and error messages.
* **Settings:** store style prompt, SKU prefix, vendor, currency, default mode, cloud opt-in.

### 2.5 Secrets and cloud (CLAUDE.md invariants 2 and 9 apply)
* Shopify credentials live in the Keychain, never in `UserDefaults`, logs, or crash reports.
* **Do not ship an Anthropic key or a Shopify client secret in the app.** The phone talks to a small backend you control (design in `docs/phase-3-shopify-live.md`, "Backend").
  Rust side: add additive constructors, for example `AnthropicGuesser::with_endpoint(base_url, bearer)` and `ShopifyStorefront::with_proxy(base_url, bearer)`.
  They are new constructors on existing objects, not trait changes; still mention them to Matt.
* Cloud is off by default. The toggle shows plainly what is sent (masked product crops, never camera frames) and to whom. Keep a visible indicator while cloud calls are possible.

### 2.6 Persistence
`sqlite_store(path)` in the app's Application Support directory; `resume()` on launch; "clear saved scan" in Settings (`clear_saved`).

### 2.7 On-device measurement
Per 5-minute session: CPU, memory peak, thermal state over time, battery drop, frames in / dropped / skipped, p50 / p95 glass-to-box latency.
Add per-stage timing to `Stats` if you need it (propose first: `Stats` / `ScanSummary` are FFI types).

## Acceptance tests

On the physical iPhone, with the 10 Phase 1 items:
1. A 5-minute session completes with no crash; thermal state never reaches `.critical`; peak memory recorded.
2. SKUs and quantities are within one item of the Mac results for sessions A and B.
3. Kill and relaunch the app mid-session; `resume()` restores the items and their states.
4. Airplane mode: scan, review, edit and CSV export all work without the network. Cloud features show as unavailable instead of failing silently.
5. Background and foreground the app during a scan without a crash or a stuck camera.
6. A Rust panic injected in a plugin (the hidden `crash_stage_for_tests` hook, debug builds only) shows up as a warning and the app recovers.
7. PLAN.md section 10 and `docs/known-issues.md` updated with measured numbers.

## Out of scope
Android, App Store submission, TestFlight, aisle walk-through, multi-user sync.
