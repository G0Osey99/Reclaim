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
    out = {
        "expected": n_expected,
        "recovered_files": n_rec,
        "content_matches": len(matched),
        "content_recall": round(len(matched) / n_expected, 4) if n_expected else 0.0,
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
