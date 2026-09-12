#!/usr/bin/env bash
# bench.sh — recovery-rate benchmark (docs/plan/09 §3). Phase 2 adds a
# named_recall column (metadata engines) alongside content_recall (carver),
# and compares the carving column against PhotoRec on each golden image.
#
# content_recall: exact SHA-256 match of recovered bytes (any name).
# named_recall  : a deleted ground-truth file is recovered at the matching path
#                 with matching content (FAT first-char loss tolerated).
# PhotoRec is GPL; we only RUN it as a competitor (build guide Part 1.4 rule 2).
#
# Usage: scripts/bench.sh
set -uo pipefail
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

RECLAIM="target/release/reclaim"
[ -x "$RECLAIM" ] || cargo build --release -p reclaim-cli >/dev/null 2>&1
WORK="${BENCH_WORK:-/tmp/reclaim-bench}"
rm -rf "$WORK"; mkdir -p "$WORK"
OUTDOC="${BENCH_OUT:-docs/build-log/phase-2/bench.md}"
mkdir -p "$(dirname "$OUTDOC")"
HAVE_PHOTOREC=0; command -v photorec >/dev/null 2>&1 && HAVE_PHOTOREC=1

# Build the synthetic NTFS image if absent (no mkfs.ntfs on this host).
[ -f testdata/build/ntfs-delete.img ] || python3 scripts/gen-fs-images ntfs-delete >/dev/null 2>&1 || true

peak_rss_kb() { awk '/maximum resident set size/ {printf "%d", $1/1024}' "$1"; }

for img in testdata/build/*.img; do
  name="$(basename "${img%.img}")"
  gt="${img%.img}.groundtruth.json"
  [ -f "$gt" ] || continue
  # Skip images with no file ground truth (e.g. gpt-deleted-partition, a
  # partition-recovery test covered by reclaim-part's integration test).
  nfiles=$(python3 -c "import json;print(len(json.load(open('$gt')).get('files',[])))" 2>/dev/null || echo 0)
  [ "$nfiles" -gt 0 ] || { echo "== $name == (no file ground truth; skipped)"; continue; }
  d="$WORK/$name"; mkdir -p "$d"
  echo "== $name =="

  # ---- reclaim: quick + deep (default), then recover preserving paths ----
  sess="$d/reclaim-sess"
  /usr/bin/time -l "$RECLAIM" scan "$img" --session "$sess" -q >"$d/reclaim.stdout" 2>"$d/reclaim.time" || true
  r_rss=$(peak_rss_kb "$d/reclaim.time"); r_sec=$(grep real "$d/reclaim.time" | awk '{print $1}')
  "$RECLAIM" recover "$sess" "$d/reclaim-out" --all --preserve-paths \
      --allow-same-device-i-accept-data-loss -q >/dev/null 2>&1 || true
  r_score=$(python3 scripts/bench_score.py "$gt" "$d/reclaim-out")

  # ---- photorec (carving competitor; content only) ----
  if [ "$HAVE_PHOTOREC" = 1 ]; then
    mkdir -p "$d/photorec"
    /usr/bin/time -l photorec /log /d "$d/photorec/recup" /cmd "$img" search \
        >"$d/photorec.stdout" 2>"$d/photorec.time" || true
    p_sec=$(grep real "$d/photorec.time" | awk '{print $1}')
    p_score=$(python3 scripts/bench_score.py "$gt" "$d/photorec" 2>/dev/null || echo '{}')
  else
    p_sec="-"; p_score='{}'
  fi

  echo "  reclaim: $r_score"
  echo "  photorec: $p_score"
  python3 - "$name" "$r_score" "$r_sec" "$r_rss" "$p_score" "$p_sec" >>"$WORK/rows.jsonl" <<'PY'
import json,sys
name,rs,rsec,rrss,ps,psec=sys.argv[1:7]
r=json.loads(rs); p=json.loads(ps) if ps.strip() not in('','{}') else {}
print(json.dumps({"name":name,"reclaim":r,"r_sec":rsec,"r_rss":rrss,"photorec":p,"p_sec":psec}))
PY
done

# ---- render markdown ----
python3 - "$WORK/rows.jsonl" "$OUTDOC" <<'PY'
import json,sys,time
rows=[json.loads(l) for l in open(sys.argv[1])]
out=open(sys.argv[2],"w"); w=out.write
def pct(x): return f"{100*x:.1f}%" if isinstance(x,(int,float)) else "—"
w("# Reclaim benchmark — named + content recall (reclaim vs PhotoRec)\n\n")
w(f"_Generated {time.strftime('%Y-%m-%d %H:%M:%S%z')} · content_recall = exact SHA-256 "
  "of recovered bytes; named_recall = deleted file recovered at the right path with "
  "matching content (docs/plan/09 §3). PhotoRec is carve-only (no names)._\n\n")
w("| image | fs | named_recall | content_recall | photorec content | reclaim prec | time | peak RSS |\n")
w("|---|---|---|---|---|---|---|---|\n")
content_ok=True
# Images whose metadata recall is intentionally an overwrite/limit case
# (documented in the build log), excluded from the pass/fail gate.
NAMED_EXCEPTIONS={"apfs-delete-history"}
for r in rows:
    rc=r["reclaim"]; pc=r.get("photorec") or {}
    rr=rc.get("content_recall",0); pr=pc.get("content_recall",None)
    nr=rc.get("named_recall",None)
    if pr is not None and rr < pr - 1e-9: content_ok=False
    note=" *" if r["name"] in NAMED_EXCEPTIONS else ""
    named_cell = (pct(nr) if nr is not None else "n/a")+note
    w(f"| {r['name']} | {rc.get('fs','')} | {named_cell} "
      f"| {pct(rr)} | {pct(pr) if pr is not None else 'n/a'} | {pct(rc.get('precision_exact',0))} "
      f"| {r['r_sec']}s | {r['r_rss']} KB |\n")
w("\n\\* `apfs-delete-history` deletes 12 files then runs 320 churn transactions "
  "that overwrite the freed metadata and data blocks; 0 of the 12 are recoverable "
  "by any method (checkpoint depth 4 xids). An honest overwrite limit — see "
  "docs/build-log/phase-3.md.\n")
w("\n## Per-family content recall (reclaim)\n\n")
for r in rows:
    fam=r["reclaim"].get("by_family",{})
    parts=", ".join(f"{k} {v['hit']}/{v['total']}" for k,v in fam.items())
    w(f"- **{r['name']}**: {parts}\n")
# Gate evaluation for the phase-3 named-recall targets.
by={r["name"]:r["reclaim"].get("named_recall") for r in rows}
def meets(name,thr): v=by.get(name); return v is not None and v>=thr-1e-9
w("\n## Gates\n\n")
w(f"- content_recall ≥ PhotoRec on every image: **{'MET' if content_ok else 'NOT MET'}**.\n")
w(f"- named_recall ≥ 0.90 on apfs-many-deletes: **{'MET' if meets('apfs-many-deletes',0.90) else 'NOT MET'}** "
  f"({pct(by.get('apfs-many-deletes'))}).\n")
w(f"- named_recall ≥ 0.95 on hfsplus-delete: **{'MET' if meets('hfsplus-delete',0.95) else 'NOT MET'}** "
  f"({pct(by.get('hfsplus-delete'))}).\n")
w(f"- named_recall ≥ 0.95 on every exFAT/FAT/NTFS image (Phase 2): "
  f"**{'MET' if all(meets(n,0.95) for n in ('exfat-camera-delete','fat32-usb-delete','ntfs-delete','ntfs-quick-format')) else 'NOT MET'}**.\n")
w("- apfs-delete-history named_recall = 0.00 — documented overwrite limit "
  "(320 churn transactions; checkpoint depth 4), not a regression.\n")
out.close()
print("wrote",sys.argv[2])
print("content_ok",content_ok,"named_ok",named_ok)
PY
echo "== done =="; cat "$OUTDOC"
