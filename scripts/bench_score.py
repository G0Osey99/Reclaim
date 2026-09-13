#!/usr/bin/env python3
"""Score a recovered-file directory against a golden-image ground-truth sidecar.

Two metrics (docs/plan/09 §3):

* content_recall — exact SHA-256 match of recovered bytes to a ground-truth
  file, independent of name. Measures the carver.
* named_recall   — for each *deleted* ground-truth file, a recovered file exists
  at the matching relative path whose bytes hash to the ground-truth file.
  Measures the metadata engines. The match allows the deleted file's first
  basename character to differ (FAT clears it on delete — docs/plan/04 §3.4);
  our engine infers it, so this relaxation almost never fires but keeps the
  metric honest where inference is impossible.

Run `reclaim recover <sess> <dir> --all --preserve-paths` before scoring so both
carved (synthesized path) and named (real path) results are present on disk.

Usage: bench_score.py <groundtruth.json> <recovered_dir>
Prints one JSON object.
"""
import hashlib
import json
import os
import sys


def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def first_char_relaxed_equal(a: str, b: str) -> bool:
    """Paths equal, or equal after ignoring the first basename character."""
    if a == b:
        return True
    da, na = os.path.split(a)
    db, nb = os.path.split(b)
    if da != db or not na or not nb:
        return False
    return na[1:] == nb[1:]


def main() -> int:
    gt_path, rec_dir = sys.argv[1], sys.argv[2]
    gt = json.load(open(gt_path))
    files = gt["files"]

    expected = {f["sha256"]: f["family"] for f in files}
    exp_by_fam = {}
    for fam in expected.values():
        exp_by_fam[fam] = exp_by_fam.get(fam, 0) + 1

    # Walk the recovered tree: collect (relpath, sha256) and a hash set.
    skip = {"manifest.json", "report.xml"}
    rec_by_path = {}        # relpath -> sha256
    rec_hashes = []
    for root, _dirs, names in os.walk(rec_dir):
        for name in names:
            if name in skip:
                continue
            p = os.path.join(root, name)
            try:
                if os.path.getsize(p) == 0:
                    continue
                h = sha256_file(p)
            except OSError:
                continue
            rel = os.path.relpath(p, rec_dir).replace(os.sep, "/")
            rec_by_path[rel] = h
            rec_hashes.append(h)

    rec_set = set(rec_hashes)

    # Precision per docs/plan/09 §3: precision = (hash_matches + valid_unknown) /
    # total_results, where a false positive is a result that matches no expected
    # file AND fails validation. `precision_exact` (below) is the stricter
    # matches-only ratio; it under-reports precision because a metadata engine
    # legitimately recovers real files that ground truth does not track (macOS
    # `._*` AppleDouble companions, `.fseventsd/*`), which are valid, not junk.
    # The recover manifest carries each result's `validity`; use it when present.
    precision_doc = None
    false_positive_rate = None
    man_path = os.path.join(rec_dir, "manifest.json")
    if os.path.exists(man_path):
        try:
            man = json.load(open(man_path))
            items = man["files"] if isinstance(man, dict) else man
            match = valid_unknown = false_pos = 0
            for it in items:
                rp = it.get("path", "")
                h = rec_by_path.get(rp)
                if h is None:
                    continue
                if h in expected:
                    match += 1
                elif it.get("validity") == "full":
                    valid_unknown += 1
                else:
                    false_pos += 1
            tot = match + valid_unknown + false_pos
            if tot:
                precision_doc = round((match + valid_unknown) / tot, 4)
                false_positive_rate = round(false_pos / tot, 4)
        except (OSError, ValueError, KeyError):
            pass

    # Content recall (any name).
    matched = {h for h in expected if h in rec_set}
    hit_by_fam = {}
    for h in matched:
        fam = expected[h]
        hit_by_fam[fam] = hit_by_fam.get(fam, 0) + 1

    # Named recall (deleted files, path + hash).
    deleted = [f for f in files if f.get("deleted")]
    named_correct = 0
    for f in deleted:
        want_path, want_hash = f["path"], f["sha256"]
        # exact path with correct content?
        got = rec_by_path.get(want_path)
        if got == want_hash:
            named_correct += 1
            continue
        # relaxed first-char path match with correct content.
        if any(first_char_relaxed_equal(want_path, rp) and h == want_hash
               for rp, h in rec_by_path.items()):
            named_correct += 1

    n_expected = len(expected)
    n_rec = len(rec_hashes)
    n_deleted = len(deleted)
    # partial_credit (docs/plan/09 §3): Σ(longest correct prefix / size) over the
    # expected set. Round 1 carves whole contiguous files only (build guide §3.6):
    # a recovered file is either an exact-hash match (credit 1.0) or its blocks
    # were overwritten/TRIM'd and nothing is present to score (credit 0.0). The
    # ground-truth sidecars retain only SHA-256, not the original bytes (privacy —
    # no corpus bytes in the repo), so a genuinely truncated survivor cannot be
    # prefix-scored; on this whole-file corpus there are none, so partial_credit
    # equals content_recall exactly. Reported so a future fragment-reassembly
    # engine (round 2) can raise it above content_recall.
    content_recall = round(len(matched) / n_expected, 4) if n_expected else 0.0
    out = {
        "expected": n_expected,
        "recovered_files": n_rec,
        "content_matches": len(matched),
        "content_recall": content_recall,
        "partial_credit": content_recall,
        # precision (docs/plan/09 §3): matches + valid_unknown / total. Falls back
        # to the matches-only ratio when no manifest is present (e.g. PhotoRec).
        "precision": precision_doc if precision_doc is not None
        else (round(len(matched) / n_rec, 4) if n_rec else 0.0),
        "false_positive_rate": false_positive_rate,
        "precision_exact": round(len(matched) / n_rec, 4) if n_rec else 0.0,
        "named_expected": n_deleted,
        "named_correct": named_correct,
        "named_recall": round(named_correct / n_deleted, 4) if n_deleted else None,
        "fs": gt.get("fs", ""),
        "by_family": {fam: {"hit": hit_by_fam.get(fam, 0), "total": exp_by_fam[fam]}
                      for fam in sorted(exp_by_fam)},
    }
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
