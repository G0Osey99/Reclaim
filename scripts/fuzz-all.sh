#!/usr/bin/env bash
# fuzz-all.sh — build and fuzz every reclaim-carve validator target
# (docs/plan/09 §4). Usage: scripts/fuzz-all.sh [seconds-per-target] [parallelism]
#
# Builds all targets once (shared sanitized deps), then runs each libFuzzer
# binary directly for the given duration in parallel. A validator that crashes
# leaves a reproducer under fuzz/artifacts/<target>/ and is reported CRASH.
set -uo pipefail
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cd "$(git rev-parse --show-toplevel)/crates/reclaim-carve"

DUR="${1:-120}"
PAR="${2:-6}"
TARGETS="jpeg png gif bmp tiff isobmff riff iff zip ole2 pdf sqlite text gzip bzip2 xz zstd sevenz rar tar macho elf pe dmg bplist mp3 flac ogg mkv mpegts iso wasm ico"
OUT="fuzz/run-logs"
mkdir -p "$OUT"
BIN_DIR="fuzz/target/aarch64-apple-darwin/release"

echo "== building all fuzz targets =="
cargo +nightly fuzz build 2>&1 | tail -3

echo "== fuzzing each target for ${DUR}s (parallel ${PAR}) =="
run_one() {
  local t="$1" dur="$2" bindir="$3" out="$4"
  mkdir -p "fuzz/corpus/$t"
  if "$bindir/$t" -max_total_time="$dur" -rss_limit_mb=4096 -print_final_stats=1 -max_len=1048576 \
       "fuzz/corpus/$t" >"$out/$t.log" 2>&1; then
    echo "PASS  $t  ($(grep -c '' "$out/$t.log") log lines)"
  else
    echo "CRASH $t  — see $out/$t.log"
  fi
}
export -f run_one
printf '%s\n' $TARGETS \
  | xargs -P "$PAR" -I{} bash -c 'run_one "$1" "'"$DUR"'" "'"$BIN_DIR"'" "'"$OUT"'"' _ {} \
  | tee "$OUT/summary.txt"

echo "== fuzz summary =="
sort "$OUT/summary.txt"
crashes=$(grep -c '^CRASH' "$OUT/summary.txt" || true)
echo "targets: $(printf '%s\n' $TARGETS | wc -w | tr -d ' ')  crashes: ${crashes:-0}"
