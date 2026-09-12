#!/usr/bin/env bash
# fuzz-fs.sh — build and run the Phase-2 partition/filesystem cargo-fuzz targets
# (docs/plan/09 §4). Requires nightly + cargo-fuzz (as in Phase 1). The in-tree
# `cargo test --test fuzz_smoke` randomized checks are the CI-runnable stand-in
# where cargo-fuzz is not installed.
#
# Usage: scripts/fuzz-fs.sh [seconds-per-target] [parallelism]
set -uo pipefail
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cd "$(git rev-parse --show-toplevel)/crates/reclaim-fs-fuzz"

DUR="${1:-120}"
PAR="${2:-4}"
TARGETS="part_scan exfat fat ntfs ntfs_record ntfs_usn"
OUT="fuzz/run-logs"
mkdir -p "$OUT"
BIN_DIR="fuzz/target/aarch64-apple-darwin/release"

if ! command -v cargo-fuzz >/dev/null 2>&1; then
  echo "cargo-fuzz not installed; run the CI stand-in instead:"
  echo "  cargo test -p reclaim-part -p fs-exfat -p fs-fat -p fs-ntfs --test fuzz_smoke"
  exit 0
fi

echo "== building fuzz targets =="
cargo +nightly fuzz build 2>&1 | tail -3

echo "== fuzzing each target for ${DUR}s (parallel ${PAR}) =="
run_one() {
  local t="$1" dur="$2" bindir="$3" out="$4"
  mkdir -p "fuzz/corpus/$t"
  if "$bindir/$t" -max_total_time="$dur" -rss_limit_mb=4096 -print_final_stats=1 \
       "fuzz/corpus/$t" >"$out/$t.log" 2>&1; then
    echo "PASS  $t"
  else
    echo "CRASH $t  — see $out/$t.log"
  fi
}
export -f run_one
printf '%s\n' $TARGETS \
  | xargs -P "$PAR" -I{} bash -c 'run_one "$1" "'"$DUR"'" "'"$BIN_DIR"'" "'"$OUT"'"' _ {} \
  | tee "$OUT/summary.txt"
crashes=$(grep -c '^CRASH' "$OUT/summary.txt" || true)
echo "targets: $(printf '%s\n' $TARGETS | wc -w | tr -d ' ')  crashes: ${crashes:-0}"
