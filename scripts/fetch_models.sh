#!/usr/bin/env bash
# Download CoreML models into models/ (git-ignored). Pinned to a Hugging Face revision and checked by sha256.
#
#   DETR-ResNet50 semantic segmentation, Apple's Core ML conversion (apple/coreml-detr-semantic-segmentation),
#   licence Apache-2.0 (model card, checked 2026-10-25). Float16, 86 MB.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO=apple/coreml-detr-semantic-segmentation
REV=7c771f8867a479d1441ac5fb0a8de31feea76bb6
PKG=DETRResnet50SemanticSegmentationF16.mlpackage
FILES=(
  "Manifest.json -"
  "Data/com.apple.CoreML/model.mlmodel 61e05678150bcae26deac589b998dffe13c2977a9ba6f802938a9f3a60bfdcce"
  "Data/com.apple.CoreML/weights/weight.bin 2f4de3dfed1aeced35ad7096d7c8721fcfb59571110a9e9138d73a7e56480408"
)
for entry in "${FILES[@]}"; do
  set -- $entry
  dst="models/$PKG/$1"
  if [ "$2" != - ] && [ -f "$dst" ] && [ "$(shasum -a 256 "$dst" | cut -d' ' -f1)" = "$2" ]; then continue; fi
  mkdir -p "$(dirname "$dst")"
  curl -fsSL "https://huggingface.co/$REPO/resolve/$REV/$PKG/$1" -o "$dst"
  if [ "$2" != - ] && [ "$(shasum -a 256 "$dst" | cut -d' ' -f1)" != "$2" ]; then echo "checksum mismatch: $dst"; rm -f "$dst"; exit 1; fi
done
echo "models/$PKG ready"
