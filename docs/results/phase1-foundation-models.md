# Pixel masks, CLIP-style embeddings and descriptions (Phase 1, Matt's request 2026-10-25)

Dev Mac: M3 Pro, macOS 26.3.1. Release builds. Models from `scripts/fetch_models.sh` (pinned, sha256-checked).
Design and licences: `design-segment-embed-describe.md`. **MobileCLIP weights are research-only** (Matt chose them for the
prototype); SAM 2.1 and DETR are Apache-2.0.

## Unit checks (`swift test -c release -Xswiftc -enable-testing`, synthetic fixtures)

| Check | Result |
|---|---|
| CLIP BPE tokenizer (Swift port) vs Hugging Face `tokenizers` 0.23.3 on 175 strings (vocabulary prompts, punctuation, digits, accents) | identical ids |
| MobileCLIP S2 image / text vectors | 512-d, unit length; same image twice cosine 1.0000 |
| Zero-shot sanity: EAN-13 card vs {barcode, cat, tree, car}; red disc vs colours/shapes; blue square vs colours/shapes | all correct, p ≈ 1.00 |
| SAM 2.1 tiny, box prompt 3 % larger than each synthetic product (box IoU 0.65 with truth) | mask box IoU **0.944 / 0.952 / 0.958**, mask fill 1.00, SAM score 1.00. Confirms prompt coordinates = pixels of the 1024² scale-filled input |
| SAMRefinedDetector over Blob proposals | 3 / 3 products, IoU > 0.85 |
| HybridGuesser gate | non-product top word ≥ 0.4 stays local; product crop goes to the (fake) cloud with the on-device top-3 in `detectorLabel`; cloud failure falls back to the local guess with a visible "Cloud: failed" attribute |

## Timing (warm, per call)

| Model | CPU + ANE | CPU + GPU | All |
|---|---|---|---|
| DETR-ResNet50 seg F16 (448²) | **29.8 ms** | 57.0 ms | 64.9 ms |
| SAM 2.1 tiny image encoder (1024²) | **120.5 ms** | 426.9 ms | 439.4 ms |
| SAM 2.1 tiny prompt encoder + mask decoder (per box) | 48.4 ms | **7.2 ms** | 8.2 ms |
| MobileCLIP S0 image (256²) | 6.8 ms | 7.0 ms | |
| MobileCLIP S2 image (256²) | **6.8 ms** | 12.3 ms | |

Defaults follow the bold column (SAM: encoder on ANE, decoder on GPU). SAM-refined detector, 3 boxes: 238 ms → **139 ms** per frame with the split.

## Live (webcam, a room with a person and furniture, no test items: speed and behaviour only)

DETR proposals → SAM masks → MobileCLIP S2 embedder + zero-shot describer, `analysis_fps` 10, 30 s.

| Frames in flight | p50 ms | p95 ms | processed / 30 s | items | UI hitches > 100 ms |
|---|---|---|---|---|---|
| 4 (default) | 881 | 1246 | 87 | 15 | 1 (548 ms) |
| 2 | 399 | 658 | 89 | 13 | 2 (422 ms) |
| **1** | 346 | **417** | 84 | 14 | 1 (448 ms) |

* Latency was queueing: ~3 frames/s throughput either way; with 4 frames queued each waits behind the others. With slow models
  use 1 frame in flight (now a setting, "Frames in flight").
* **Unexplained ~450 ms main-thread hitch** with this stack (not seen with Blob / Vision / DETR alone). Not yet diagnosed.
  Also: Apply builds the engine (model loads, ~200 vocabulary text embeddings) on the main thread, so the UI freezes for a
  few seconds then. Both to fix.
* Descriptions of the room (zero-shot, ~200-word vocabulary): the person's balaclava / hoodie came out as "White scarf",
  "Black hat", "Black jacket"; also "Black person", "Black face", "Orange curtain", "Basket", "Black computer monitor",
  and wrong ones ("chocolate bar", "hammer", "paint tube"). One person becomes several items (face, hat, jacket).
  Matt's verdict: "not the greatest classification". Next: Claude names products (HybridGuesser), CLIP gates and hints.
