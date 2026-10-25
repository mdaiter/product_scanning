# Handoff to Claude Code (on Matt's Mac)

You are picking up a Rust library that was built and tested in a Linux sandbox. The portable core (Phase 0) is done and
compiles. Everything that needs a Mac, an iPhone, a camera, or a real Shopify store is still ahead, and none of it has run.
Read in this order: this file, `CLAUDE.md` (rules and invariants), `PLAN.md` (the spec, decisions, verified / unverified table),
then the phase document you are on.

## The product in one paragraph

Point a phone camera at a table (v1) or a store aisle (later), and the app indexes each distinct product, proposes what it is,
lets the user confirm or correct (checkmark, X, typed correction, "hide this", flip for a barcode), drafts **one Shopify listing per product**,
and publishes them as **DRAFT**. Rust core with UniFFI bindings; actors are `ractor` on `tokio`; plugins (detector, embedder,
barcode scanner, guesser, lookup, writer, price advisor, storefront) are traits that Swift, Kotlin, Python or Rust can implement.

## What is true right now (measured in the sandbox, Linux x86_64)

| Check | Result |
|---|---|
| `cargo test --workspace --all-features` | 54 unit + 24 pipeline tests pass |
| Pipeline suite stability | 8 of 8 consecutive runs green after fixing one racy assertion (events are asynchronous; see `docs/events.md`) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean |
| `scripts/python_smoke.sh` | passes: plugins written in Python called from Rust, typed errors cross the boundary, async calls work, built-in Rust plugins can be handed over from Python |
| Swift bindings | `uniffi-bindgen-swift` generates `shelfscan.swift` (about 280 KB), a C header and a modulemap from `libshelfscan.a`. **Not compiled with swiftc.** |
| CLI | `--source synthetic`, `--source images --input DIR` (also `--review`) and `--source ffmpeg --input file.mp4` run end to end; they write `products.csv`, `catalog.json`, `crops/` |
| Mock-server tests | Anthropic guesser / writer, Open Food Facts, Shopify `productSet` (including client-credentials token fetch / cache / 401 refresh), Shopify CSV (parsed back with the `csv` crate) |
| Shopify field names | checked against shopify.dev for API `2026-10` (see `docs/phase-3-shopify-live.md`) |

What has **never** run: anything on a Mac or iPhone; the ffmpeg path with a live webcam (`avfoundation`; only a video file has been tried); Apple Vision / CoreML; the Swift sketches in `apple/`;
any live Anthropic or Shopify call; Shopify's CSV importer; Open Food Facts live. PLAN.md section 10 is the authoritative list.

## First 30 minutes on the Mac

```bash
# 0. toolchain
xcode-select --install; curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh; brew install ffmpeg
# 1. the repo is not under git yet
cd shelfscan && git init && git add -A && git commit -m "Phase 0 snapshot"
# 2. the baseline must be green before you change anything
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
bash scripts/python_smoke.sh
# 3. the demo
cargo run -p shelfscan-cli -- --source synthetic --out out/ && open out/
# 4. first contact with Apple tooling (expect failures; this is the point)
bash scripts/build_xcframework.sh && (cd apple && swift build)
```

If step 2 is not green on the Mac, stop and report it. If step 4 fails, that is Phase 1.2; fix it there.

## Where things are

```
crates/shelfscan/src/
  engine.rs        EngineBuilder + Engine (the UniFFI surface; owns the tokio runtime and the credit counter)
  traits.rs        plugin traits (FFI)          types.rs  FFI records/enums/config   error.rs  ShelfError
  actors/          catalog (state machine, SOLE writer of item state), track, detect, embed, barcode, guess, overlay,
                   lists, publish, pricing, events, supervisor, msg (all message types)
  backends/        built-ins: blob_detector, color_embedder, rxing_scanner, placeholder (guesser+writer), price stub, strategy
  cloud/           anthropic (guesser+writer), off (Open Food Facts), http, mock (test server)       [feature net]
  storefront/      csv, shopify (productSet DRAFT; client credentials or static token)               [shopify: feature net]
  store/           Store trait, memory, sqlite (FTS5 + vector + RRF)
  synthetic.rs     deterministic test scenes and a real EAN-13 renderer
crates/shelfscan-cli/        demo binary         crates/uniffi-bindgen{,-swift}/   binding generators
bindings/python/             smoke test          apple/                            Swift package + UNCOMPILED sketches
docs/                        phase specs, events contract, known issues
```

## How to work

* **Phase gates.** Do not start phase N+1 until phase N's acceptance tests pass and Matt has seen the results. Each phase doc ends with its tests.
* **Evidence over taste.** Detector and embedder choices come from the bake-offs in Phase 1, recorded in `docs/results/`.
* **Keep PLAN.md section 10 honest.** When something becomes verified, say how (what ran, where). When it fails, say that. Never mark a Vision / CoreML / iPhone / live-Shopify item verified from reading docs.
* **Ask Matt before** (from CLAUDE.md): changing a trait signature or FFI record, adding a dependency with native code, touching the state machine (`actors/catalog.rs`), changing the privacy rules, adding events. Several Phase 1 items need this; the phase doc marks them "PROPOSE FIRST". Write a short design note, then ask.
* **Invariants** (CLAUDE.md): listings are DRAFT; cloud sees masked crops only and only when the app opted in; `Catalog` is the single writer; plugin traits stay synchronous; `panic = "unwind"`; quantity never goes to Shopify inventory unless opted in; no invented prices or descriptions.
* **Tests.** New behaviour gets a test next to it. Synthetic frames come from `shelfscan::synthetic`. Tests never touch the network (use `cloud::mock::MockServer`). If you assert on events, wait for `Event::ScanFlushed` first.
* **Commits.** Small, one concern each. No secrets in the repo or in logs; the token and key code paths are written so errors never echo them, keep it that way.

## Live output (a question Matt asked)

Results come back as events on an `EventListener`, not as return values; `Overlay` carries boxes for drawing over the preview. Full contract, threading rules and the
latency measurement are in `docs/events.md`. The CLI has no video window; the Mac demo app in Phase 1.4 is the live view.

## Questions for Matt (ask early; they block later work)

1. Which iPhone model and iOS version, and which Apple Developer team / bundle id? (Phase 2.) The manifest assumes iOS 17+.
2. Is there an existing Shopify store or legacy custom app, or do we start from a Dev Dashboard app and a dev store created in the same organization? (Phase 3.0.)
3. Where could a small backend proxy run (holds the Anthropic key and Shopify credentials)? Needed before anyone else uses the phone app. (Phase 3.7.)
4. Are the proposed acceptance numbers in `docs/phase-1-mac.md` right? Can he supply the 10 test items and run the CSV import click-through?
5. Is running the Anthropic cloud quality / privacy audit (Phase 1.11) allowed on his key and with his items?
6. Model licences: OK to bundle CLIP / SigLIP / DINOv2-class weights if their licences allow it? (Check each; do not assume.)
7. Android timing: after Phase 3, or not planned?

## Document map

| File | What it is |
|---|---|
| `README.md` | what the repo is, quick start |
| `CLAUDE.md` | working rules and invariants |
| `PLAN.md` | spec of record: decisions, architecture, state machine, verified / unverified table |
| `docs/phase-1-mac.md` | Mac: Vision plugins, live preview, dataset, bake-offs, calibration, CSV import |
| `docs/phase-2-iphone.md` | iPhone app shell, lifecycle, secrets, on-device measurement |
| `docs/phase-3-shopify-live.md` | live Shopify, images, idempotency, backend design |
| `docs/events.md` | the event / live-UI contract |
| `docs/known-issues.md` | limits and gaps, with which phase fixes them |
