//! JPEG marker walker (docs/plan/05 §4.3).
//!
//! SOI → APPn/DQT/DHT/SOF/COM (length-prefixed, skipped by length) → SOS →
//! entropy-coded scan (advance to the next `FF xx`, `xx ∉ {00, D0..D7}`) → EOI.
//! Reaching EOI is `Full`; running out of data is `Truncated`. Skipping
//! length-prefixed segments (rather than scanning for `FF D9`) is what makes a
//! stray `FF D9` inside a COM/entropy payload harmless — the exact end is the
//! real EOI marker. EXIF (APP1) yields date/model/thumbnail for naming.

use super::{Ctx, Verdict};
use crate::bytes::be_u16;
use crate::validators::tiff;
use crate::Validity;

/// Largest JPEG we will walk (docs/plan/05 catalog cap).
const MAX: u64 = 256 * 1024 * 1024;
/// Entropy-scan window.
const SCAN_WIN: usize = 1 << 20;
/// Guard against pathological marker loops (fuzzing).
const MAX_MARKERS: u32 = 200_000;

/// Validate a JPEG candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 4);
    if head.first() != Some(&0xFF) || head.get(1) != Some(&0xD8) {
        return Verdict::reject();
    }
    let cap = ctx.available().min(MAX);
    if cap < 4 {
        return Verdict::reject();
    }

    let mut pos: u64 = 2;
    let mut saw_sof = false;
    let mut saw_sos = false;
    let mut meta = crate::metadata::Metadata::default();

    for _ in 0..MAX_MARKERS {
        if pos + 2 > cap {
            return truncated(pos, saw_sof);
        }
        let m = ctx.read(pos, 4);
        let b0 = m.first().copied().unwrap_or(0);
        if b0 != 0xFF {
            // Fill/desync: hunt forward for the next marker prefix.
            match next_marker(ctx, pos, cap) {
                Some(p) => {
                    pos = p;
                    continue;
                }
                None => return truncated(pos, saw_sof),
            }
        }
        let marker = m.get(1).copied().unwrap_or(0);
        match marker {
            0xD9 => {
                // EOI.
                let len = pos + 2;
                let score = if saw_sof && saw_sos { 96 } else { 82 };
                return Verdict::accept(len, Validity::Full, score).with_meta(meta);
            }
            0xFF => {
                // Fill byte: only one 0xFF is consumed.
                pos += 1;
            }
            0x01 | 0xD0..=0xD7 => {
                pos += 2;
            }
            0xD8 => {
                // A second SOI is the next file: this one ended here.
                return truncated(pos, saw_sof);
            }
            0xDA => {
                // SOS: skip its header, then scan entropy data for next marker.
                saw_sos = true;
                let seg = be_u16(&m, 2).unwrap_or(0) as u64;
                if seg < 2 {
                    return truncated(pos, saw_sof);
                }
                pos = pos + 2 + seg;
                match next_marker(ctx, pos, cap) {
                    Some(p) => pos = p,
                    None => return truncated(pos, saw_sof),
                }
            }
            0xC0..=0xCF if marker != 0xC4 && marker != 0xC8 && marker != 0xCC => {
                // SOF frame header.
                saw_sof = true;
                let seg = be_u16(&m, 2).unwrap_or(0) as u64;
                if seg < 2 {
                    return truncated(pos, saw_sof);
                }
                pos = pos + 2 + seg;
            }
            0xE1 => {
                // APP1: try EXIF.
                let seg = be_u16(&m, 2).unwrap_or(0) as u64;
                if seg < 2 {
                    return truncated(pos, saw_sof);
                }
                if meta.is_empty() {
                    extract_exif(ctx, pos + 4, seg.saturating_sub(2), &mut meta);
                }
                pos = pos + 2 + seg;
            }
            _ => {
                // Any other length-prefixed segment (APPn/DQT/DHT/COM/…).
                let seg = be_u16(&m, 2).unwrap_or(0) as u64;
                if seg < 2 {
                    return truncated(pos, saw_sof);
                }
                pos = pos + 2 + seg;
            }
        }
    }
    truncated(pos, saw_sof)
}

/// Scan forward from `pos` for the next real marker prefix (`FF xx`,
/// `xx ∉ {00, D0..D7, FF}`), returning its absolute rel position.
fn next_marker(ctx: &Ctx, mut pos: u64, cap: u64) -> Option<u64> {
    while pos < cap {
        let want = (cap - pos).min(SCAN_WIN as u64) as usize;
        let buf = ctx.read(pos, want);
        if buf.is_empty() {
            return None;
        }
        let mut i = 0usize;
        while i + 1 < buf.len() {
            if buf.get(i) == Some(&0xFF) {
                let nx = buf.get(i + 1).copied().unwrap_or(0);
                if nx != 0x00 && nx != 0xFF && !(0xD0..=0xD7).contains(&nx) {
                    return Some(pos + i as u64);
                }
            }
            i += 1;
        }
        // Re-scan the last byte in case FF straddles the window boundary.
        if want < SCAN_WIN {
            break;
        }
        pos += (want - 1) as u64;
    }
    None
}

fn truncated(pos: u64, saw_sof: bool) -> Verdict {
    if pos < 107 {
        return Verdict::reject();
    }
    let score = if saw_sof { 55 } else { 35 };
    Verdict::accept(pos, Validity::Truncated, score)
}

/// Parse an EXIF APP1 payload (`Exif\0\0` + TIFF) for date/model/thumbnail.
fn extract_exif(ctx: &Ctx, rel_off: u64, seg_len: u64, meta: &mut crate::metadata::Metadata) {
    let n = seg_len.min(128 * 1024) as usize;
    let payload = ctx.read(rel_off, n);
    if payload.len() < 8 || payload.get(..6) != Some(b"Exif\x00\x00") {
        return;
    }
    if let Some(tiff_bytes) = payload.get(6..) {
        let m = tiff::parse_exif(tiff_bytes, ctx.file_start + rel_off + 6);
        *meta = m;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    fn ctx_for<'a>(r: &'a MemReader<'a>) -> Ctx<'a> {
        Ctx {
            reader: r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("image.jpeg").unwrap(),
        }
    }

    /// Build a synthetic JPEG like the golden-image generator: SOI + APP0 +
    /// COM (with a stray FF D9 inside its payload) + EOI.
    fn synthetic_jpeg() -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        // APP0 JFIF
        let app0 = b"JFIF\x00\x01\x02\x00\x00\x01\x00\x01\x00\x00";
        v.extend_from_slice(&[0xFF, 0xE0]);
        v.extend_from_slice(&((app0.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(app0);
        // COM with a decoy FF D9 in the payload — must NOT be treated as EOI.
        let payload = [0x00u8, 0xFF, 0xD9, 0x11, 0x22];
        v.extend_from_slice(&[0xFF, 0xFE]);
        v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(&payload);
        // pad to exceed min size
        v.extend_from_slice(&[0xFF, 0xFE]);
        v.extend_from_slice(&(200u16 + 2).to_be_bytes());
        v.extend_from_slice(&[0x55u8; 200]);
        v.extend_from_slice(&[0xFF, 0xD9]); // real EOI
        v
    }

    #[test]
    fn walks_to_real_eoi_ignoring_decoy() {
        let data = synthetic_jpeg();
        let end = data.len() as u64;
        let mut padded = data;
        padded.extend_from_slice(&[0xAB; 4096]); // trailing garbage after EOI
        let r = MemReader::new(&padded);
        let v = validate(&ctx_for(&r));
        assert!(v.accept);
        assert_eq!(v.validity, Validity::Full);
        assert_eq!(v.len, end, "must stop at the real EOI, not the decoy");
    }

    #[test]
    fn rejects_non_jpeg() {
        let data = vec![0u8; 4096];
        let r = MemReader::new(&data);
        assert!(!validate(&ctx_for(&r)).accept);
    }
}
