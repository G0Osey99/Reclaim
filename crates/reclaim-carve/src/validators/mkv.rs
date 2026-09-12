//! Matroska / WebM (docs/plan/05 §3.3): EBML header + Segment element. When the
//! Segment carries a known size, the file end is exact; unknown-size (live)
//! streams fall back to a bounded `Suspect` carve.

use super::{Ctx, Verdict};
use crate::Validity;

const CAP: u64 = 16 * 1024 * 1024 * 1024;

/// Validate a Matroska candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 4);
    if head != [0x1A, 0x45, 0xDF, 0xA3] {
        return Verdict::reject();
    }
    // EBML header element size (VINT) follows the 4-byte id.
    let sz_bytes = ctx.read(4, 8);
    let (ebml_size, n1) = match read_vint(&sz_bytes) {
        Some(v) => v,
        None => return Verdict::reject(),
    };
    let ebml_size = ebml_size.unwrap_or(0);
    let seg_id_pos = 4 + n1 as u64 + ebml_size;
    let seg = ctx.read(seg_id_pos, 12);
    if seg.get(0..4) != Some(&[0x18, 0x53, 0x80, 0x67]) {
        // Segment not found where expected → still a Matroska, bounded.
        return Verdict::accept(
            ctx.available().min(CAP).min(64 * 1024 * 1024),
            Validity::Suspect,
            45,
        )
        .with_format(refine(ctx));
    }
    let seg_size_bytes = seg.get(4..12).unwrap_or(&[]);
    let (seg_size, n2) = match read_vint(seg_size_bytes) {
        Some(v) => v,
        None => return Verdict::reject(),
    };
    let data_start = seg_id_pos + 4 + n2 as u64;
    let id = refine(ctx);
    match seg_size {
        Some(s) => {
            let total = data_start + s;
            let avail = ctx.available();
            if total <= avail {
                Verdict::accept(total, Validity::Full, 88).with_format(id)
            } else {
                Verdict::accept(avail, Validity::Truncated, 50).with_format(id)
            }
        }
        None => Verdict::accept(ctx.available().min(CAP), Validity::Suspect, 50).with_format(id),
    }
}

fn refine(_ctx: &Ctx) -> &'static str {
    // WebM and MKV share the container id; the catalog groups them under one id.
    "video.mkv"
}

/// Read an EBML VINT size. Returns (value, consumed); value None ⇒ unknown-size
/// (all data bits set). Returns outer None on malformed input.
fn read_vint(b: &[u8]) -> Option<(Option<u64>, usize)> {
    let first = *b.first()?;
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || b.len() < len {
        return None;
    }
    // Strip the length-marker bit from the first byte.
    let mut val = u64::from(first & (0xFF >> len));
    let mut all_ones = (first & (0xFF >> len)) == (0xFF >> len);
    for i in 1..len {
        let byte = *b.get(i)?;
        val = (val << 8) | u64::from(byte);
        if byte != 0xFF {
            all_ones = false;
        }
    }
    if all_ones {
        Some((None, len))
    } else {
        Some((Some(val), len))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn known_segment_size() {
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
        v.push(0x84); // EBML header size vint = 4
        v.extend_from_slice(&[0u8; 4]); // ebml header body
        v.extend_from_slice(&[0x18, 0x53, 0x80, 0x67]); // Segment id
        v.push(0x88); // size vint length 1 → value 8
        v.extend_from_slice(&[0u8; 8]);
        let end = v.len() as u64;
        v.extend_from_slice(&[0u8; 32]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("video.mkv").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, end);
    }
}
