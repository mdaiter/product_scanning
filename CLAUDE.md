# CLAUDE.md

Rules for working in this repo. Start with `HANDOFF.md`, then `PLAN.md` (the spec, and the list of what is verified).

## Layout

```
crates/shelfscan/            core library (lib + cdylib + staticlib). All UniFFI exports live here.
crates/shelfscan-cli/        demo binary `shelfscan` (synthetic / image folder / ffmpeg pipe; review loop; CSV + Shopify)
crates/uniffi-bindgen/       `cargo run -p uniffi-bindgen -- generate ...` (Python, Kotlin, ...)
crates/uniffi-bindgen-swift/ `cargo run -p uniffi-bindgen-swift -- ...` (Swift sources, headers, modulemap)
bindings/python/             smoke test that implements plugins in Python
apple/                       Swift package + UNCOMPILED sketches (Vision plugins, camera source, overlay model)
docs/                        phase specs, events contract, known issues
scripts/                     python_smoke.sh, build_xcframework.sh (Mac only, never run yet)
```

## Commands

```
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
bash scripts/python_smoke.sh                                   # build, generate Python bindings, run the plugin round trip
cargo run -p shelfscan-cli -- --source synthetic --out out/
cargo run -p shelfscan-cli -- --source images --input ./photos --review
bash scripts/build_xcframework.sh                              # Mac: xcframework + Swift sources into apple/
```

## Invariants (do not break without asking)

1. Listings are `DRAFT`. No code path sets a listing `ACTIVE` (the engine forces `Draft`; storefronts send `DRAFT` / `draft`).
2. Cloud calls send **masked product crops only**, never full frames, and only when the app
   constructed a cloud plugin. Tests never touch the network; use `cloud::mock::MockServer`.
3. `Catalog` is the single writer of item state. Everything else sends it messages.
4. Plugin traits stay synchronous. Blocking work runs through `spawn_blocking`.
5. `panic = "unwind"` in release profiles (actor supervision depends on it).
6. `sqlite-vec` is not a dependency (cosine runs in Rust). If it is ever added, pin `=0.1.9`; the alpha line failed to build.
7. Prices and descriptions are never invented silently: unknown fields are marked
   "needs your input" in the draft.
8. FFI-visible types live in `types.rs`, `traits.rs`, `error.rs`. Use UniFFI proc macros, not UDL.
9. Quantity is an estimate; do not write it to Shopify inventory unless the caller opts in.
10. Secrets (API keys, client secrets, access tokens) never appear in error messages, events, logs or the repo.
    The HTTP helper and storefronts are written and tested so that errors cannot echo them; keep it that way.
11. Events are asynchronous and ordered; `Overlay` and `Stats` are lossy. Do not build logic on counting events right after
    `flush()`; wait for `Event::ScanFlushed` (see `docs/events.md`).

## Working style

* Say what is verified and what is not. Anything touching a real Shopify store, a phone, Vision,
  CoreML or a live API stays labelled UNVERIFIED in PLAN.md section 10 until it has actually run.
  Reading vendor docs counts as "docs", not as verified.
* Write tests next to the code; synthetic frames come from `shelfscan::synthetic`.
* Ask before: changing a trait signature or an FFI record, adding an event, adding a dependency with native code,
  touching the state machine (`actors/catalog.rs`), or changing the privacy rules above.
  Adding a constructor to an existing object is fine; mention it.
* Phase gates are in PLAN.md section 9 and `docs/phase-*.md`. Do not start a phase before the previous one's
  acceptance tests pass and Matt has seen the results.
* Record experiment results in `docs/results/` (create it) and summarise them in PLAN.md section 10.
* Keep commits small and single-purpose.
