//! Recoverability scoring (build guide Phase-2 prompt B).
//!
//! > Recoverability score = validator/structure confidence × unallocated-extents
//! > fraction × not-overwritten.
//!
//! We fold "unallocated-extents fraction" and "not-overwritten" into the same
//! measurement: a deleted file whose blocks are now free is both unallocated and
//! un-overwritten, so the bitmap's free fraction over the file's extents is the
//! single multiplier. The result is a 0..100 integer to match the carver's
//! `score` field so named and carved results sort together.

use crate::{Bitmap, Entry, EntryState};

/// Compute the recoverability score (0..100) for an entry.
///
/// * `structure_conf` is the engine's structural confidence (0..1).
/// * `bitmap` (when present) decides the unallocated/not-overwritten fraction
///   over the entry's extents; without a bitmap we assume a deleted file's
///   space is free (fraction 1.0) but cap the score a little for the unknown.
#[must_use]
pub fn recoverability(entry: &Entry, bitmap: Option<&Bitmap>) -> u8 {
    let structure = entry.confidence.clamp(0.0, 1.0);

    // Live files are fully present; report high confidence directly.
    if entry.state == EntryState::Live {
        return to_score(structure * 0.99);
    }

    let free_fraction = match bitmap {
        Some(bm) if !entry.extents.is_empty() => {
            // Byte-weighted mean free fraction across the extents.
            let mut weighted = 0.0f32;
            let mut total = 0u64;
            for e in &entry.extents {
                let f = bm.unallocated_fraction(e.offset, e.len);
                weighted += f * (e.len as f32);
                total = total.saturating_add(e.len);
            }
            if total == 0 {
                1.0
            } else {
                weighted / (total as f32)
            }
        }
        // No bitmap, or no extents: assume free but acknowledge the unknown.
        _ => 0.85,
    };

    let mut s = structure * free_fraction;
    // A chain we had to assume contiguous is less trustworthy (Suspect).
    if entry.contiguous_assumed {
        s *= 0.7;
    }
    to_score(s)
}

fn to_score(f: f32) -> u8 {
    let v = (f.clamp(0.0, 1.0) * 100.0).round();
    v as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Extent;

    fn del_entry(conf: f32) -> Entry {
        let mut e = Entry::new_file(1, "x".into(), b"x".to_vec(), 8192, EntryState::Deleted);
        e.confidence = conf;
        e.extents = vec![Extent {
            offset: 0x10000,
            len: 8192,
        }];
        e
    }

    #[test]
    fn free_space_scores_high() {
        // 16 clusters, 0..2 allocated, 3.. free. Extent at 0x10000 covers
        // clusters 0,1 (allocated) — so low score.
        let bits = vec![0b0000_0111u8, 0u8];
        let bm = Bitmap::new(4096, 0x10000, 16, bits);
        let e = del_entry(1.0);
        let s = recoverability(&e, Some(&bm));
        assert!(s < 10, "allocated extent should score low, got {s}");

        // Extent in free space.
        let mut e2 = del_entry(1.0);
        e2.extents = vec![Extent {
            offset: 0x10000 + 4 * 4096,
            len: 8192,
        }];
        let s2 = recoverability(&e2, Some(&bm));
        assert!(s2 >= 90, "free extent should score high, got {s2}");
    }

    #[test]
    fn contiguous_assumed_penalised() {
        let mut e = del_entry(1.0);
        e.contiguous_assumed = true;
        let s = recoverability(&e, None);
        // 1.0 * 0.85 * 0.7 ≈ 0.595
        assert!((55..=65).contains(&s), "got {s}");
    }

    #[test]
    fn live_is_high() {
        let mut e = del_entry(0.9);
        e.state = EntryState::Live;
        assert!(recoverability(&e, None) >= 88);
    }
}
