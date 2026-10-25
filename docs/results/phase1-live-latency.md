# 1.4 Live Mac demo: latency and UI measurements

Dev Mac (M3 Pro, macOS 26.3.1), `apple/Examples/MacDemo` release build, `--measure 30`, FaceTime HD Camera at 1920x1080
(the 1280x720 session preset was not what the camera delivered), `analysis_fps` 10, `analysis_max_side` 960.
Scene: a room with a person in front of the camera, **no test items** (the 1.5 table is not set up yet). These numbers
measure the pipeline's speed, not detection quality.

Glass-to-box latency = `hostClockMs() - Overlay.timestampMs` in the listener (docs/events.md); "UI" = the same once the
main actor applied the boxes. Hitch = a gap over 100 ms in a 60 Hz main-thread timer, armed at the first frame.

| Plugins (detector / embedder / barcode) | p50 ms | p95 ms | UI p95 ms | hitches | frames in / processed / skipped / dropped | detections | items |
|---|---|---|---|---|---|---|---|
| Blob / Color / rxing (2026-10-25, run 1) | 73 | 82 | 82 | 0 (worst 28 ms) | 288 / 222 / 66 / 0 | 1 | 0 |
| Blob / Color / rxing (run 2) | 78 | 82 | 83 | 0 (worst 80 ms) | 307 / 228 / 79 / 0 | 0 | 0 |
| Vision foreground (masks) / feature print / Vision barcode | 115 | 122 | 122 | 0 (worst 18 ms) | 292 / 105 / 187 / 0 | 104 | 1 |
| Vision foreground (no masks) / feature print / Vision barcode | 107 | 113 | 114 | 0 (worst 30 ms) | 298 / 198 / 100 / 0 | 197 | 1 |

Observations:
* p95 is well under the 500 ms acceptance limit with every plugin set; no UI hitch while scanning; no frames dropped
  (the rate gate and the novelty gate skip frames before they are copied).
* **Blob finds nothing in a room.** It thresholds against the border colour and needs a plain table; it is a CI / demo fallback.
* **Vision foreground reported the person as a product** ("Unidentified black product", barcode prompt). Foreground
  masks find salient objects, not products. Nothing left the Mac (no cloud plugin). Whether to mark people as
  Background is a design question (roles, not drop filters): see the open items in PROGRESS.md.
* The first live run showed boxes mirrored vertically. Cause: the overlay sublayer used AppKit's bottom-left
  origin (`isGeometryFlipped` is not inherited). Fixed, plus an off-screen self-test (`--overlay-test --selftest`)
  that fails the build script if a known box lands in the wrong place (it reported y 724...906 before the fix, 94...276 after).
* Overlay geometry is now correct by the self-test, but the boxes have **not yet been checked against real objects on the screen**.
