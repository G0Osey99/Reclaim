#!/usr/bin/env bash
# bench.sh — recovery-rate benchmark: reclaim vs PhotoRec on the golden images
# (docs/plan/09 §3). Writes docs/build-log/phase-1/bench.md.
#
# Content recall is measured by exact SHA-256 match against each image's
# ground-truth sidecar, identically for both tools. PhotoRec is GPL; we only
# RUN it as a competitor (build guide Part 1.4 rule 2), never read its code.
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
OUTDOC="docs/build-log/phase-1/bench.md"
mkdir -p "$(dirname "$OUTDOC")"
HAVE_PHOTOREC=0; command -v photorec >/dev/null 2>&1 && HAVE_PHOTOREC=1

# max RSS (KB) from `/usr/bin/time -l` stderr (macOS reports bytes).
peak_rss_kb() { awk '/maximum resident set size/ {printf "%d", $1/1024}' "$1"; }
elapsed_s()  { awk '/real/ {print $1; exit} END{}' "$1" 2>/dev/null; }

rows=""
for img in testdata/build/*.img; do
  name="$(basename "${img%.img}")"
  gt="${img%.img}.groundtruth.json"
  [ -f "$gt" ] || continue
  d="$WORK/$name"; mkdir -p "$d"
  echo "== $name =="

  # ---- reclaim ----
  sess="$d/reclaim-sess"
  /usr/bin/time -l "$RECLAIM" scan --deep "$img" --session "$sess" -q >"$d/reclaim.stdout" 2>"$d/reclaim.time" || true
  r_rss=$(peak_rss_kb "$d/reclaim.time"); r_sec=$(grep real "$d/reclaim.time" | awk '{print $1}')
  "$RECLAIM" recover "$sess" "$d/reclaim-out" --all --flat \
      --allow-same-device-i-accept-data-loss -q >/dev/null 2>&1 || true
  r_score=$(python3 scripts/bench_score.py "$gt" "$d/reclaim-out")

  # ---- photorec ----
  if [ "$HAVE_PHOTOREC" = 1 ]; then
    mkdir -p "$d/photorec"
    /usr/bin/time -l photorec /log /d "$d/photorec/recup" /cmd "$img" search \
        >"$d/photorec.stdout" 2>"$d/photorec.time" || true
    p_rss=$(peak_rss_kb "$d/photorec.time"); p_sec=$(grep real "$d/photorec.time" | awk '{print $1}')
    p_score=$(python3 scripts/bench_score.py "$gt" "$d/photorec" 2>/dev/null || echo '{}')
  else
    p_rss="-"; p_sec="-"; p_score='{}'
  fi

  echo "  reclaim: $r_score"
  echo "  photorec: $p_score"
  # stash for the report writer
  python3 - "$name" "$r_score" "$r_sec" "$r_rss" "$p_score" "$p_sec" "$p_rss" >>"$WORK/rows.jsonl" <<'PY'
import json,sys
name,rs,rsec,rrss,ps,psec,prss=sys.argv[1:8]
r=json.loads(rs); p=json.loads(ps) if ps.strip() not in('','{}') else {}
print(json.dumps({"name":name,"reclaim":r,"r_sec":rsec,"r_rss":rrss,
                  "photorec":p,"p_sec":psec,"p_rss":prss}))
PY
done

# ---- render markdown ----
python3 - "$WORK/rows.jsonl" "$OUTDOC" <<'PY'
import json,sys,time
rows=[json.loads(l) for l in open(sys.argv[1])]
out=open(sys.argv[2],"w")
w=out.write
w("# Phase 1 benchmark — reclaim vs PhotoRec\n\n")
w(f"_Generated {time.strftime('%Y-%m-%d %H:%M:%S%z')} · exact SHA-256 content recall "
  "against each image's ground-truth sidecar (docs/plan/09 §3)._\n\n")
w("| image | reclaim recall | photorec recall | reclaim prec | reclaim time | reclaim peak RSS | photorec time |\n")
w("|---|---|---|---|---|---|---|\n")
def pct(x): return f"{100*x:.1f}%" if isinstance(x,(int,float)) else "—"
allpass=True
for r in rows:
    rc=r["reclaim"]; pc=r.get("photorec") or {}
    rr=rc.get("content_recall",0); pr=pc.get("content_recall",None)
    if pr is not None and rr < pr - 1e-9: allpass=False
    w(f"| {r['name']} | {pct(rr)} | {pct(pr) if pr is not None else 'n/a'} "
      f"| {pct(rc.get('precision_exact',0))} | {r['r_sec']}s | {r['r_rss']} KB | "
      f"{(r['p_sec']+'s') if r['p_sec']!='-' else 'n/a'} |\n")
w("\n## Per-family content recall (reclaim)\n\n")
for r in rows:
    fam=r["reclaim"].get("by_family",{})
    parts=", ".join(f"{k} {v['hit']}/{v['total']}" for k,v in fam.items())
    w(f"- **{r['name']}**: {parts}\n")
w("\n## Notes\n\n")
w("- Target (build-guide Phase-1 gate): reclaim `content_recall` ≥ PhotoRec on every image — "
  f"**{'MET' if allpass else 'NOT MET'}**.\n")
w("- Partial credit (longest-correct-prefix) needs the original file bytes; the sidecar stores "
  "only hashes, so it is reported N/A this phase.\n")
w("- Text (`.txt`) exact recall depends on cluster slack being zeroed; where PhotoRec and reclaim "
  "both carve headerless text statistically, neither reproduces an exact hash when the on-disk "
  "extent differs from the recorded size.\n")
out.close()
print("wrote",sys.argv[2],"— target",("MET" if allpass else "NOT MET"))
PY
echo "== done =="; cat "$OUTDOC"
