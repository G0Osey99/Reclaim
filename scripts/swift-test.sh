#!/usr/bin/env bash
# swift-test.sh — run the Swift view-model + golden-image tests (build guide
# Phase-5 D). XCTest/Swift Testing are unavailable under Command Line Tools, so
# these run as an assertion executable (ReclaimSelfTest) rather than
# `xcodebuild test`; see docs/build-log/phase-5.md. Ready-to-run XCTest sources
# for a full-Xcode `xcodebuild test` live in apps/Reclaim/XcodeTests/.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=13.0

# Ensure a fresh debug FFI lib + bindings are present.
scripts/gen-ffi-bindings.sh debug >/dev/null

export RECLAIM_GOLDEN="$(pwd)/testdata/build/exfat-camera-delete.img"
cd apps/Reclaim
swift run ReclaimSelfTest 2>&1 | grep -vE "ld: warning|was built for newer"
