# Live output: how a UI follows a running camera

`push_frame` never returns results. It returns `true` (frame accepted) or `false` (dropped at the edge), and
everything the pipeline learns comes back as **events** on the `EventListener` you registered. Register it on
the builder (`EngineBuilder.listener(...)`) or later (`engine.add_listener(...)`).

```
camera thread                    engine (tokio, own threads)                      your UI
-------------                    ---------------------------                      -------
wants_frame(ts)? --no--> skip (no copy, camera never blocks)
        |yes
push_frame(frame) -> true/false  Ingest -> Detect -> Track -> Embed -> Catalog
                                                   |            |          |
                                                   +--> Overlay actor      +--> Guess / Lookup tasks
                                                                |
                                                     Events actor (one blocking task per event, in order)
                                                                |  listener.on_event(event)   <-- engine thread
                                                                v
                                                     hop to the main thread / main actor, update state
```

## Delivery rules (tested in `tests/pipeline.rs`)

* **Asynchronous.** `flush().await` returning does *not* mean the listener has seen the events yet. Wait for
  `Event::ScanFlushed` before asserting on earlier events. (One of our own tests raced on exactly this.)
* **Ordered.** All events go through one actor and one at a time, so if you have seen event B you have seen every
  event emitted before it (except dropped lossy ones, below).
* **Lossy by design: `Overlay` and `Stats`.** At most 2 of them are queued; more are dropped. A slow UI sees fewer
  box updates, never a growing backlog. Every other event is delivered.
* **Listener must be quick.** Events are delivered one at a time on the blocking pool. A listener that blocks
  delays every later event, and non-lossy events queue without a bound while it does. Copy what you need and hop to
  the UI thread (`Task { @MainActor in ... }`, `DispatchQueue.main.async`, `runOnUiThread`).
* **A throwing or panicking listener is contained** (`catch_unwind`); other listeners still run.
* **Not thread-affine.** Do not assume the callback runs on the thread that created the engine.

## Events a UI needs

| Event | When | Use |
|---|---|---|
| `Overlay { frame_id, timestamp_ms, boxes, hidden }` | per processed frame (lossy) | draw boxes over the preview; `hidden` feeds the "N hidden" chip |
| `OverlayBox { track_id, bbox (0..1), item_id?, state?, role, label, needs_barcode, focused }` | inside `Overlay` | colour by state; `label` is ready-made text ("Looks like: red cereal box?") |
| `ItemDiscovered`, `ItemUpdated { quantity }` | new product / quantity change | add / refresh a row |
| `GuessReady { item_id, title, confidence }` | a guess arrived | show the proposal for ✓ / ✗ |
| `ItemStateChanged { item_id, state }` | any state-machine move | row status |
| `BarcodeRequested { item_id, hint }` | catalog mode wants the barcode | "flip it over and show the barcode" |
| `BarcodeRead { item_id, gtin }` | a validated GTIN (2 agreeing frames) | clear the prompt |
| `ItemMerged { from_id, into_id }` | two items turned out to share a GTIN | remove `from_id`'s row |
| `PossibleDuplicate { item_id, existing_id, similarity }` | looks like an earlier session's item | optional "already scanned?" hint |
| `ScanFlushed { items }` | `flush()` completed | end-of-scan marker |
| `ListingDrafted`, `PublishProgress { done, total }` | listing / publish work | progress UI |
| `Warning`, `Error { source, message }` | plugin threw, actor restarted, ... | non-fatal toast / log |
| `Stats { summary, tracks_open }` | periodic (lossy) | frames in / dropped / skipped, items |

## Coordinates

`bbox` is normalised (0..1) against the frame **as the engine received it**, after your rotation. If you rotate the
frame for portrait before `push_frame`, boxes are in portrait space. Mapping onto a preview layer that crops
(aspect-fill) needs the same crop applied; the sketch in `apple/Sources/ShelfScanCapture` assumes aspect-fit.

## Measuring latency (Phase 1 acceptance uses this)

`timestamp_ms` on `Overlay` is the capture timestamp you stamped on the frame. In the listener, record
`now_ms - timestamp_ms` using the same clock; report p50 / p95. That is the glass-to-box latency.

## Current CLI

`shelfscan --source ffmpeg ...` prints discovery / guess / barcode / warning lines as they happen (stderr). It has no
video window. A live preview is a Phase 1 task (Mac app first, then the same code on iPhone).
