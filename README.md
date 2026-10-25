# shelfscan

Scan a table or a store with a camera, index the distinct products, draft one Shopify listing per product, let the user
confirm or correct, publish as **drafts**. Rust core, UniFFI bindings (Swift, Kotlin, Python), `ractor` actors on `tokio`.
iPhone first.

**Status:** the portable core works and is tested on Linux. Nothing has run on a Mac, an iPhone, or against a live Shopify store yet.
Start with [`HANDOFF.md`](HANDOFF.md).

## Try it

```bash
cargo test --workspace --all-features
cargo run -p shelfscan-cli -- --source synthetic --out out/          # no camera, no network
cargo run -p shelfscan-cli -- --source images --input ./photos --review
bash scripts/python_smoke.sh                                           # plugins written in Python
```

Output in `out/`: `products.csv` (Shopify import, every row `draft`), `catalog.json`, `crops/`.
Cloud is opt-in: `--anthropic` (reads `ANTHROPIC_API_KEY`), `--off-contact EMAIL`, `--shopify-shop NAME`
(reads `SHOPIFY_ADMIN_TOKEN`, or `SHOPIFY_CLIENT_ID` + `SHOPIFY_CLIENT_SECRET`).

## Use as a library

```rust
let engine = EngineBuilder::new(EngineConfig::default())
    .detector(my_detector).embedder(my_embedder)      // any Arc<dyn Detector> etc.; Swift/Kotlin/Python can implement them
    .listener(my_listener)                            // events come back here: boxes, guesses, barcode prompts
    .sqlite_store("shelf.db".into())?
    .build()?;
if engine.wants_frame(ts_ms) { engine.push_frame(frame); }   // never blocks; false means dropped
engine.flush().await?;                                        // wait for the scan to settle
engine.confirm_item("item-001".into()).await?;
let listings = engine.generate_listings(false).await?;       // always DRAFT
engine.publish(ids, CsvStorefront::new("products.csv".into(), false)).await?;
```

## Layout and docs

See the document map in [`HANDOFF.md`](HANDOFF.md). Rules for contributors (human or agent): [`CLAUDE.md`](CLAUDE.md). Spec: [`PLAN.md`](PLAN.md).

Licence: declared as `MIT OR Apache-2.0` in `Cargo.toml`; there are no LICENSE files yet (Matt to confirm the licence).
# product_scanning
