#!/usr/bin/env python3
"""Score a recovered-file directory against a golden-image ground-truth sidecar.

Content recall is measured by exact SHA-256 match (docs/plan/09 §3): a recovered
file counts when its hash equals a ground-truth file's hash, independent of name.
Precision here is exact-match precision (hash-matching files / data files
recovered). Partial credit (longest correct prefix) needs the original bytes,
which the sidecar does not store (only hashes) — reported N/A; see phase-1.md.

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


def main() -> int:
    gt_path, rec_dir = sys.argv[1], sys.argv[2]
    gt = json.load(open(gt_path))
    # These recipes only delete (no overwrite), so every file's data is intact
    # on disk → the whole set is the recoverable expectation.
    expected = {f["sha256"]: f["family"] for f in gt["files"]}
    exp_by_fam = {}
    for fam in expected.values():
        exp_by_fam[fam] = exp_by_fam.get(fam, 0) + 1

    skip = {"manifest.json", "report.xml"}
    recovered_hashes = []
    for root, _dirs, files in os.walk(rec_dir):
        for name in files:
            if name in skip:
                continue
            p = os.path.join(root, name)
            try:
                if os.path.getsize(p) == 0:
                    continue
                recovered_hashes.append(sha256_file(p))
            except OSError:
                pass

    rec_set = set(recovered_hashes)
    matched = {h for h in expected if h in rec_set}
    hit_by_fam = {}
    for h in matched:
        fam = expected[h]
        hit_by_fam[fam] = hit_by_fam.get(fam, 0) + 1

    n_expected = len(expected)
    n_rec = len(recovered_hashes)
    recall = len(matched) / n_expected if n_expected else 0.0
    precision = len(matched) / n_rec if n_rec else 0.0
    print(json.dumps({
        "expected": n_expected,
        "recovered_files": n_rec,
        "content_matches": len(matched),
        "content_recall": round(recall, 4),
        "precision_exact": round(precision, 4),
        "by_family": {fam: {"hit": hit_by_fam.get(fam, 0), "total": exp_by_fam[fam]}
                      for fam in sorted(exp_by_fam)},
    }))
    return 0


if __name__ == "__main__":
    sys.exit(main())
