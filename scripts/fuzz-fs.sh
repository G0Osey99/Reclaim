#!/usr/bin/env bash
# fuzz-fs.sh — build and run the Phase-2 partition/filesystem cargo-fuzz targets
# (docs/plan/09 §4). Requires nightly + cargo-fuzz (as in Phase 1). The in-tree
# `cargo test --test fuzz_smoke` randomized checks are the CI-runnable stand-in
# where cargo-fuzz is not installed.
#
# The fuzz crate is consolidated at crates/reclaim-fs-fuzz (its own workspace,
# excluded from the main workspace), so cargo-fuzz is invoked with --fuzz-dir.
#
# Usage: scripts/fuzz-fs.sh [seconds-per-target] [parallelism]
set -uo pipefail
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/rustup/bin:$PATH"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

DUR="${1:-135}"
PAR="${2:-6}"
FUZZ_DIR="crates/reclaim-fs-fuzz"
# Full set across phases; override with FUZZ_TARGETS="ext iso container structs".
TARGETS="${FUZZ_TARGETS:-part_scan exfat fat ntfs ntfs_record ntfs_usn apfs apfs_record hfs hfs_record ext iso container structs}"
OUT="$FUZZ_DIR/run-logs"
mkdir -p "$OUT"
BIN_DIR="$FUZZ_DIR/target/aarch64-apple-darwin/release"

if ! command -v cargo-fuzz >/dev/null 2>&1; then
  echo "cargo-fuzz not installed; run the CI stand-in instead:"
  echo "  cargo test -p reclaim-part -p fs-exfat -p fs-fat -p fs-ntfs --test fuzz_smoke"
  exit 0
fi

echo "== building fuzz targets =="
cargo +nightly fuzz build --fuzz-dir "$FUZZ_DIR" 2>&1 | tail -3

echo "== fuzzing each target for ${DUR}s (parallel ${PAR}) =="
run_one() {
  local t="$1" dur="$2" bindir="$3" out="$4" fdir="$5"
  mkdir -p "$fdir/corpus/$t"
  if "$bindir/$t" -max_total_time="$dur" -rss_limit_mb=4096 -print_final_stats=1 -max_len=1048576 \
       "$fdir/corpus/$t" >"$out/$t.log" 2>&1; then
    echo "PASS  $t"
  else
    echo "CRASH $t  — see $out/$t.log"
  fi
}
export -f run_one
printf '%s\n' $TARGETS \
  | xargs -P "$PAR" -I{} bash -c 'run_one "$1" "'"$DUR"'" "'"$BIN_DIR"'" "'"$OUT"'" "'"$FUZZ_DIR"'"' _ {} \
  | tee "$OUT/summary.txt"
crashes=$(grep -c '^CRASH' "$OUT/summary.txt" || true)
echo "targets: $(printf '%s\n' $TARGETS | wc -w | tr -d ' ')  crashes: ${crashes:-0}"
