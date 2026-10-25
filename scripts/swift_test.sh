#!/usr/bin/env bash
# Regenerate the xcframework + Swift bindings, write test fixtures, then build and test the Swift package on macOS.
# Set SKIP_XCFRAMEWORK=1 to reuse an existing apple/build (faster when only Swift changed).
set -euo pipefail
cd "$(dirname "$0")/.."
[ "${SKIP_XCFRAMEWORK:-0}" = 1 ] || bash scripts/build_xcframework.sh
cargo run -q -p shelfscan-cli -- fixtures --out apple/Tests/ShelfScanTests/Fixtures
cd apple && swift build && swift test
# Mac demo: build, then check overlay geometry (renders a known box off-screen; no camera needed)
cd Examples/MacDemo && swift build -c release && .build/release/MacDemo --overlay-test --selftest | grep SELFTEST
