# Design note: pixel masks → image embedder → zero-shot description

Status: **proposal**, 2026-10-25. Asked by Matt after the first live runs. Licences checked on the vendors' pages on this date (docs, not legal advice).

## Why the current detectors pick up a person

None of them knows what a product is.
* `VisionForegroundDetector` is Apple's "lift the subject" request: it is built to cut people and pets out of photos. A person
  in front of the camera is the most salient subject, so it is reported.
* `CoreMLSegmentationDetector` (DETR) has a COCO `person` class and reports it with that label (it can now be skipped by label).
* The engine turns every detection into a product candidate unless its box is larger than `background_area`. Roles, not drop
  filters (PLAN decision 6), so nothing is discarded without a reason the user can see.

## Proposal (fits the existing traits; no FFI or trait change)

```
frame ─► Detector: class-agnostic proposals ─► SAM 2.1 tiny refines each box to a pixel mask ─► Detection { bbox, mask, label }
                                                                                                     │ engine cuts out onto white (cutout_on_white)
masked crop ─► Embedder: SigLIP 2 image tower (CoreML) ─► unit vector ─► SKU clustering (calibrated in 1.8)
masked crop ─► ProductGuesser: same image vector · text vectors of a product vocabulary ─► "Looks like: ceramic mug?" + confidence
```

* **Proposals:** start with what exists (Vision foreground instances, DETR thing regions, focus-mode centre box); SAM refines
  their boxes to pixel masks. This is MobileSAMv2's idea (object-aware prompts instead of a 32x32 point grid) without its
  YOLOv8 prompt generator (AGPL). A full point grid needs one decoder run per point: too slow for live use; keep it for stills.
* **Embedder and describer share one image forward pass**: the guesser can reuse the vector (cache by crop hash), so description
  costs one dot product against pre-computed text embeddings (a few hundred product nouns / attributes, computed once).
* **People:** with a text tower we can also score "a person" against the crop and give it the Background role (needs the roles
  call to accept a detector hint: today the label is passed through; a role hint would be an FFI change → ask first).

## Candidate models (CoreML)

| Model | Use | Licence (source) | CoreML build | Size |
|---|---|---|---|---|
| SAM 2.1 tiny (`apple/coreml-sam2.1-tiny`) | box/point → pixel mask | Apache-2.0 (HF model card) | Apple's conversion (image encoder, prompt encoder, mask decoder) | 80 MB |
| SigLIP 2 base (`google/siglip2-base-patch16-224`) | image + text embeddings | Apache-2.0 (HF model card) | none official; **convert ourselves** with coremltools (community conversions exist, provenance unknown) | ~375 MB fp32 / ~190 MB fp16, both towers |
| DINOv2 small (`facebook/dinov2-small`) | image embedding only (no text) | Apache-2.0 | convert ourselves | ~90 MB |
| OpenAI CLIP ViT-B/32 | image + text | MIT (GitHub repo; HF card has no licence tag) | convert ourselves | ~600 MB fp32 |
| MobileCLIP S0–B (`apple/coreml-mobileclip`) | image + text, fastest (S0 image ≈ 1.5 ms) | **Research only**: "does not include any commercial exploitation, product development or use in any commercial product" (LICENSE_MODELS) | Apple's conversion | 23 MB image + 85 MB text (S0) |

Recommendation: SAM 2.1 tiny + SigLIP 2 base (both Apache-2.0). MobileCLIP only if shelfscan stays a research prototype.
Everything is measured in the 1.8 embedder bake-off (ROC AUC on same/different crops) before it becomes a default.

## Needs Matt
1. Is shelfscan commercial (rules out MobileCLIP)?
2. OK to create a local Python venv with torch + transformers + coremltools (~2 GB download, outside the repo) to convert
   SigLIP 2 / DINOv2? The converter script and pinned revisions go in `scripts/`, the weights in git-ignored `models/`.
