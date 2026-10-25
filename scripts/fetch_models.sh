#!/usr/bin/env bash
# Download Core ML models into models/ (git-ignored). Each file is pinned to a Hugging Face revision and checked by sha256.
# Usage: scripts/fetch_models.sh [detr] [sam] [mobileclip]     (no argument = all)
#
# Licences (read on the model cards / repos, 2026-10-25):
#   detr        apple/coreml-detr-semantic-segmentation   Apache-2.0                                   86 MB
#   sam         apple/coreml-sam2.1-tiny                  Apache-2.0                                   80 MB
#   mobileclip  apple/coreml-mobileclip (S0 + S2)         Apple ML Research Model licence: RESEARCH USE ONLY,
#               no product development or commercial use (github.com/apple/ml-mobileclip LICENSE_MODELS). ~300 MB
#               + CLIP BPE tokenizer files from openai/clip-vit-base-patch32 (OpenAI CLIP, MIT)              1.4 MB
set -euo pipefail
cd "$(dirname "$0")/.."

fetch() {   # repo rev package relative-file sha256|- [destination]
  local dst="${6:-models/$3/$4}"
  if [ "$5" != - ] && [ -f "$dst" ] && [ "$(shasum -a 256 "$dst" | cut -d' ' -f1)" = "$5" ]; then return; fi
  mkdir -p "$(dirname "$dst")"
  curl -fsSL "https://huggingface.co/$1/resolve/$2/${3:+$3/}$4" -o "$dst"
  if [ "$5" != - ] && [ "$(shasum -a 256 "$dst" | cut -d' ' -f1)" != "$5" ]; then echo "checksum mismatch: $dst"; rm -f "$dst"; exit 1; fi
}
pkg() {     # repo rev package model.mlmodel-sha weight.bin-sha
  fetch "$1" "$2" "$3" Manifest.json -
  fetch "$1" "$2" "$3" Data/com.apple.CoreML/model.mlmodel "$4"
  fetch "$1" "$2" "$3" Data/com.apple.CoreML/weights/weight.bin "$5"
  echo "models/$3 ready"
}

want() { [ $# -eq 0 ] && return 0; for a in "${ARGS[@]}"; do [ "$a" = "$1" ] && return 0; done; return 1; }
ARGS=("$@")

if [ ${#ARGS[@]} -eq 0 ] || want detr; then
  pkg apple/coreml-detr-semantic-segmentation 7c771f8867a479d1441ac5fb0a8de31feea76bb6 DETRResnet50SemanticSegmentationF16.mlpackage \
    61e05678150bcae26deac589b998dffe13c2977a9ba6f802938a9f3a60bfdcce 2f4de3dfed1aeced35ad7096d7c8721fcfb59571110a9e9138d73a7e56480408
fi
if [ ${#ARGS[@]} -eq 0 ] || want sam; then
  R=39ae0a8a83e5e6cd196e804bf7cccc5f8171f306
  pkg apple/coreml-sam2.1-tiny $R SAM2_1TinyImageEncoderFLOAT16.mlpackage \
    6cbc50301ee3ff4a9366083f9647e1f06762759542d8dd0fac394ebc3682cce7 eab96eb8ff35720c79eedc0cac2a4ef32d685f9c994c39736027078528c48a97
  pkg apple/coreml-sam2.1-tiny $R SAM2_1TinyPromptEncoderFLOAT16.mlpackage \
    3a83c167d8bd63e80f86349a78c2ab0527ce97eca1f848a4ce57fe5351241fa3 af466cf28ef8838f409c2bfd8cc0049b9efbf9db335d60a57dbfc5160af883f2
  pkg apple/coreml-sam2.1-tiny $R SAM2_1TinyMaskDecoderFLOAT16.mlpackage \
    4601f302d4c6936e15de3a22089c2afe1fa009ef703f82147ff829b4be677577 f5a8635981199fa1199007ed6798c61a326288548b74553b3c2ddb932fcdc8de
fi
if [ ${#ARGS[@]} -eq 0 ] || want mobileclip; then
  echo "MobileCLIP weights: research use only (Apple ML Research Model licence)."
  R=3e0a7bfb9fe83da8a3efaa3fd8f7df24214bb947
  pkg apple/coreml-mobileclip $R mobileclip_s0_image.mlpackage \
    2c1afa132c41c6535817cc67894bd7484bc2cbd084ed5e2f12b24f611af17591 87d8f63997bbd2f38ba7defeaaa2c571928bdece56aa9629542198b3ce906ed6
  pkg apple/coreml-mobileclip $R mobileclip_s0_text.mlpackage \
    81eba836ff4dbc8ae021d70006288b533ba7eed3c2973d245b0d5ea047305bfd 34723e51445b2630106e94e1fdbebed80e7676b404fb839f4eb9bec97bdcad68
  pkg apple/coreml-mobileclip $R mobileclip_s2_image.mlpackage \
    2aeb3359f6cde65e9f9248ec2a742e9939bd4bbf48c2f55fcd255b4504d96a1b 6cbc7fb06b6072c1cae9c4496d67e0e6217adbf726dfeb82e44d4efe87c34c00
  pkg apple/coreml-mobileclip $R mobileclip_s2_text.mlpackage \
    b8651b6d030bae419a9548b41c8fae11f96b59cfa21b6e532a4c4434522b4b80 8e8d5454f104b6cbb58d98bf11e038ff1f1943599efea111260a832f094cd0ce
  T=3d74acf9a28c67741b2f4f2ea7635f0aaf6f0268
  fetch openai/clip-vit-base-patch32 $T "" vocab.json 5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363 models/clip-tokenizer/vocab.json
  fetch openai/clip-vit-base-patch32 $T "" merges.txt f526393189112391ce6f9795d4695f704121ce452c3aad1f5335cc41337eba85 models/clip-tokenizer/merges.txt
  echo "models/clip-tokenizer ready"
fi
