#!/usr/bin/env bash
# check-readonly.sh — static read-only proof (docs/plan/09 §5, build guide Part 3.2).
#
# Greps all tracked Rust source for byte-writing / device-mutating APIs and fails
# if any appears in a file NOT on the allow-list. The allow-list
# (scripts/readonly-allowlist.txt) is reviewed on every change; only the imager
# destination, the session store and the read-only device-geometry ioctls belong
# on it. A hit anywhere else is a P0 (build guide Part 1.4 rule 1).
#
# Written for bash 3.2 (stock macOS) — no mapfile / associative arrays. Rust
# paths never contain spaces, so word-splitting the file list is safe.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

ALLOWLIST="scripts/readonly-allowlist.txt"

# Patterns that can write bytes to a file/device or mutate a device. Kept narrow
# so ordinary `Write` to Vec/stdout/String and read-only `File::open`/`libc::open`
# do not trip it.
PATTERNS='File::create
OpenOptions
\.write\(true\)
\.append\(true\)
\.create\(true\)
\.create_new\(
fs::write
fs::remove_
libc::write
libc::pwrite
libc::ftruncate
libc::unlink
libc::ioctl
\bpwrite\b
\bioctl\b'

# Build the allow-list into an extended-regex of path prefixes.
allow_re=""
if [ -f "$ALLOWLIST" ]; then
    while IFS= read -r line; do
        line="${line%%#*}"                              # strip comments
        line="$(printf '%s' "$line" | xargs 2>/dev/null || true)"  # trim ws
        [ -z "$line" ] && continue
        if [ -n "$allow_re" ]; then allow_re="$allow_re|$line"; else allow_re="$line"; fi
    done < "$ALLOWLIST"
fi

RS_FILES="$(git ls-files '*.rs')"
if [ -z "$RS_FILES" ]; then
    echo "check-readonly: no Rust sources tracked yet — nothing to scan."
    exit 0
fi

violations=0
for pat in $PATTERNS; do
    # shellcheck disable=SC2086  # RS_FILES intentionally word-split (no spaces in Rust paths)
    while IFS= read -r hit; do
        [ -z "$hit" ] && continue
        file="${hit%%:*}"
        if [ -n "$allow_re" ] && printf '%s' "$file" | grep -qE "^($allow_re)"; then
            continue   # allow-listed file
        fi
        echo "READ-ONLY VIOLATION [$pat]: $hit"
        violations=$((violations + 1))
    done <<EOF
$(grep -nHE "$pat" $RS_FILES 2>/dev/null || true)
EOF
done

if [ "$violations" -gt 0 ]; then
    echo ""
    echo "check-readonly: $violations write/mutate pattern(s) found outside the allow-list."
    echo "If a hit is a legitimate destination/session write, add its path to $ALLOWLIST with a reason."
    exit 1
fi

echo "check-readonly: OK — no write/mutate patterns outside the allow-list."
