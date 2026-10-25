# Phase 3: live Shopify (and the backend the phone needs)

**Goal.** Drafts created in a real Shopify store from real scans, with images, correct fields, no duplicates, readable errors.
Everything about Shopify so far was checked against shopify.dev and tested against a local mock, never against a store.

**Gate in.** Phase 1 acceptance passed (Phase 2 is not required for 3.0 to 3.5; they run from the Mac CLI).

**Gate out.** Acceptance tests below.

## What the docs confirmed (shopify.dev, API version 2026-10; read 2026-10-24)

* `productSet(input: ProductSetInput!, synchronous: Boolean)`; `synchronous` defaults to true and then returns `product`; with false it returns a `productSetOperation`. Needs scope `write_products`. Payload: `product`, `productSetOperation`, `userErrors { field message code }`. There is also an `identifier: ProductSetIdentifiers` argument ("used to look up the resource").
* `ProductStatus` has `ACTIVE`, `ARCHIVED`, `DRAFT`, `UNLISTED`. We send `DRAFT` only.
* `ProductSetInput`: `title`, `descriptionHtml`, `handle`, `vendor`, `productType`, `tags`, `status`, `seo` (`SEOInput { title, description }`), `productOptions` (max 3), `variants`, `files`, `metafields`, `category`, ...
* `ProductVariantSetInput`: `optionValues` (required), `sku`, `price`, `compareAtPrice`, `inventoryQuantities { locationId, name, quantity }`, `inventoryPolicy`, `file`, `taxable`, ... **`barcode` is deprecated; use `barcodes: [{ value, type? }]`** (up to 20, not combinable with `barcode`). The code already uses `barcodes`.
* `FileSetInput`: `originalSource`, `contentType`, `alt`, `filename`, `duplicateResolutionMode`. `originalSource` accepts public URLs; for staged uploads the page names video and 3D model.
* `stagedUploadsCreate`: input `resource` (`PRODUCT_IMAGE`, `VIDEO`, `MODEL_3D`, ...), `filename`, `mimeType`, `httpMethod`, `fileSize`; returns `stagedTargets { url resourceUrl parameters { name value } }`. The page says to upload to `url` with the returned parameters and then use `resourceUrl` as `originalSource` in `fileCreate` or `productUpdate`. It does not mention `productSet`.
* **Auth changed.** Since 2026-01-01 you cannot create new legacy custom apps (existing ones keep working). New apps are made in the Dev Dashboard. For stores in the same organization as the app, the **client credentials grant** works: `POST https://{shop}.myshopify.com/admin/oauth/access_token`, form body `grant_type=client_credentials&client_id=...&client_secret=...`, response `access_token`, `scope`, `expires_in` (86399 s, so 24 h tokens). A dev store created from the Shopify admin is *not* in your org; create it from the Dev Dashboard's Dev stores page. Stores outside the org need custom distribution with token exchange or the authorization-code grant (that needs a backend).

## Still UNVERIFIED (test these first)

* That a `productSet` with `status: DRAFT`, `sku`, `barcodes`, `seo`, tags, vendor, default option works as built (`ShopifyStorefront::product_input`).
* Whether `price: "0.00"` is accepted; whether duplicate titles collide on `handle` (we do not send a handle).
* What `productSet` does when the `identifier` matches nothing (create or error?). It matters for idempotent retries.
* Whether `files[].originalSource` accepts a `stagedUploadsCreate` `resourceUrl` for images inside `productSet`.
* `inventoryQuantities` needs a location and a tracked inventory item; scope names for it (`write_inventory`, `read_locations`).
* GraphQL cost / throttle behaviour for 100 sequential `productSet` calls.
* The scopes needed for `fileCreate` / staged uploads (`write_files`?).

## Work items

### 3.0 Credentials and a dev store
1. Dev Dashboard: create an app with scopes `write_products` (add others only when a work item needs them), release a version.
2. Dev Dashboard -> Dev stores: create a development store **in the same organization**; install the app on it.
3. Run the CLI: `SHOPIFY_CLIENT_ID=... SHOPIFY_CLIENT_SECRET=... shelfscan --source images --input ./photos --review --shopify-shop my-store`.
   (`SHOPIFY_ADMIN_TOKEN` still works if Matt already has a legacy custom app.) Matt creates the app and store; Claude Code never needs his Shopify login, only the client id and secret in the environment.

### 3.1 First live drafts
Publish 3 listings. Read every `userErrors` message. For each fix: change `ShopifyStorefront`, add or adjust the mock test so the regression is covered,
and note the finding in `docs/results/phase3-shopify.md`. Then verify in the admin: status Draft, not on Online Store, SKU, barcode, vendor, type, tags, SEO fields.

### 3.2 Images
1. `stagedUploadsCreate` (`resource: PRODUCT_IMAGE`, `httpMethod: POST`, filename, `mimeType: image/jpeg`, `fileSize`).
2. Multipart POST to the returned `url`: the returned `parameters` first, in order, then the file part last. ureq 3 has no multipart helper; build the body by hand (boundary, parts) in `cloud/http.rs` (`send_multipart`) and cover it with the mock server.
3. Attach: try `productSet` with `files: [{ originalSource: resourceUrl, contentType: IMAGE, alt, filename }]`. If Shopify rejects staged URLs there, create the product first and then `fileCreate` / `productUpdate` with the `resourceUrl` as the docs describe.
4. Decide image quality: listing images are small masked crops. If they look poor on a storefront, add a high-resolution still capture per confirmed item (a Phase 2 UI task plus an `images` source in the engine; propose first).
`ListingImage { jpeg, alt }` is already carried on `Listing.images`; `product_input` ignores it today.

### 3.3 Inventory (opt-in only)
Query `locations`, let the caller pick one, send `inventoryQuantities`. Confirm the scopes and that the variant's inventory item is tracked. Quantity stays an estimate (CLAUDE.md invariant 9): the default remains "do not touch inventory".

### 3.4 Idempotency and ambiguous failures
The engine marks a listing published only after a successful receipt and refuses to publish it twice. A lost response (timeout after Shopify created the product) can still create a duplicate on retry.
Options, in order of preference once tested: `productSet` with `identifier: { handle }` or `customId` so a retry updates instead of creating; or look up by SKU / handle before creating. Pick one by experiment; document it.

### 3.5 Throughput
Publish 100 synthetic listings (the CLI can generate them) and record time, throttling responses, and how `publish_chunk_size` (default 10) behaves.

### 3.6 CSV path
If not done in Phase 1.12, import `products.csv` and record results. The CSV remains the fallback for people without Admin API access.

### 3.7 Backend (design now, build only if Matt asks)
Needed before anyone but Matt uses the phone app, because a client secret or Anthropic key on a phone is extractable and Shopify tokens expire every 24 h.
Minimal service: authenticates the app install (per-install token), holds the Shopify client credentials or OAuth tokens per shop and the Anthropic key,
exposes `POST /v1/vision/messages` (pass-through to the Anthropic Messages API, rate-limited per install) and `POST /v1/shopify/products` (takes a `Listing` JSON, performs the `productSet`, returns a `PublishReceipt`).
The Rust side gets `AnthropicGuesser::with_endpoint(...)` and `ShopifyStorefront::with_proxy(...)` constructors that point at it. Stores outside Matt's organization additionally need the app's custom distribution and the authorization-code flow.

## Acceptance tests

1. The 10 listings from the Phase 1 scan appear in the dev store as **Draft**, with correct title, SKU (GTIN where known, else `PREFIX-seq`), barcode, vendor, tags, SEO fields, and images; none is visible on the storefront.
2. Publishing the same listings again does not create duplicates (or the engine refuses with a clear message).
3. A bad input (for example an invalid price) produces a failed receipt with Shopify's `field: message` text; other listings in the batch still succeed.
4. A token that expires mid-run is refreshed and the run completes (unit-tested with the mock; confirm once live).
5. Inventory is untouched unless a location id was passed.
6. 100 listings publish without unhandled throttling errors.
7. All mock tests updated for what the live store taught us; clippy and the test suite pass; PLAN.md section 10 updated: every Shopify line is now "verified live" or still marked UNVERIFIED with a reason.
