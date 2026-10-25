# Known issues and limits (as of the handoff)

Honest list. "Fix in" says which phase should pick it up.

## Perception quality

* **Default embedder is colour-only** (`ColorEmbedder`, hue-bin grid). Different products with the same colours will merge
  (one checkmark then confirms both), and hue-bin boundaries add noise to reds. Fix in Phase 1 (bake-off, section 1.8).
* **Default detector is classical** (`BlobDetector`: border-median background subtraction + connected components). It merges
  touching products and needs a plain background. It exists for CI and demos. Fix in Phase 1 (Vision foreground masks or a CoreML detector).
* **No camera-motion compensation in Track.** Fine for tabletop with a steady phone; an aisle walk would lose tracks
  (measured on the sample reel: median camera shift 6.6% of width between analysed frames). Stretch goal.
* **Similarity thresholds are placeholders** (`sku_similarity` 0.92, `fixture_similarity` 0.95, `duplicate_similarity` 0.97).
  Calibrate per embedder on labelled pairs (Phase 1). Category gate and keypoint/OCR verification (PLAN section 5, steps 2 and 4) are not built.
* **Quantity** is the maximum number of concurrently open tracks of an item. It is an estimate; it is never written to Shopify inventory unless you opt in.
* **Listing images are small masked crops** (a by-product of analysis), not storefront-quality photos. Phase 3 should decide whether to add a
  high-resolution still capture per confirmed item.

## State and persistence

* Fixture library entries cannot be removed once added (no API yet).
* After a `Catalog` actor restart, only the current session's items are restored (from the Store key `session_items`).
* Listing drafts live in the `Lists` actor; if that actor is restarted by the supervisor they are lost (regenerate them).
* `PossibleDuplicate` is only a hint; nothing is merged across sessions automatically.
* One option per CSV export; the Admin API storefront supports up to 3 options (Shopify's limit). The engine does not group variants yet (one listing per product).

## Concurrency and delivery

* `ractor` mailboxes are unbounded (observed in 0.16.x). Backpressure is the credit counter at `push_frame`; do not push from inside a listener callback.
* Events are asynchronous; see `docs/events.md`.
* A slow `EventListener` delays all later events.
* Swift 6 strict concurrency: UniFFI's Swift 6 support is partial and exported `async` functions are not `Sendable`
  (uniffi-rs issue 2448). Expect warnings or `@preconcurrency` imports.

## Network backends

* Anthropic and Shopify code has only run against local mock servers. See PLAN.md section 10 for what the docs confirmed.
* The Anthropic default model name `claude-sonnet-5-5` is configurable (`model` argument).
* Shopify: no image upload, no inventory unless a location id is given, no duplicate protection if a publish response is lost in transit.
* Shopify auth: legacy custom-app tokens can no longer be created for new setups (since 2026-01-01). `ShopifyStorefront::with_client_credentials` implements the Dev Dashboard client-credentials grant (24 h tokens; same organization only).
* Open Food Facts data is ODbL (attribution and share-alike); GS1's free tier is limited.

## Tests

* The pipeline suite takes about 22 s. It passed 8 of 8 consecutive runs after one racy assertion was fixed; run it a few times after changes to timing-sensitive code.
* Nothing has run on an iPhone, a Mac, or Android. The Python smoke test is the only foreign-language check that has executed.
