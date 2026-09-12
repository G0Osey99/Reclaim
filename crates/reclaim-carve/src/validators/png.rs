//! PNG chunk walker with CRC-32 validation (docs/plan/05 §4).
//!
//! `\x89PNG\r\n\x1a\n` → repeated `[len(4 BE)][type(4)][data(len)][crc(4)]`
//! chunks, each CRC-checked over type+data, to `IEND`. Per-chunk CRCs make
//! this validation very strong: the exact end is the byte after IEND's CRC,
//! and a CRC failure pinpoints the corrupt/overwritten chunk (→ `Truncated`).

use super::{Ctx, Verdict};
use crate::bytes::be_u32;
use crate::crc::crc32;
use crate::Validity;

const MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const MAX: u64 = 256 * 1024 * 1024;
/// Cap on per-chunk bytes read for CRC (larger chunks are trusted, → Suspect).
const CRC_CAP: usize = 16 * 1024 * 1024;
const MAX_CHUNKS: u32 = 500_000;

/// Validate a PNG candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    if ctx.read(0, 8) != MAGIC {
        return Verdict::reject();
    }
    let cap = ctx.available().min(MAX);
    let mut pos: u64 = 8;
    let mut first = true;
    let mut suspect = false;

    for _ in 0..MAX_CHUNKS {
        let hdr = ctx.read(pos, 8);
        if hdr.len() < 8 {
            return truncated(pos, first);
        }
        let len = be_u32(&hdr, 0).unwrap_or(u32::MAX) as u64;
        let typ = hdr.get(4..8).unwrap_or(&[]);
        // Chunk total = 4 (len) + 4 (type) + len (data) + 4 (crc).
        let chunk_total = 12u64.saturating_add(len);
        if pos.saturating_add(chunk_total) > cap {
            return truncated(pos, first);
        }
        if first && typ != b"IHDR" {
            return Verdict::reject();
        }

        // Verify CRC over type+data (bounded).
        let want_crc = {
            let c = ctx.read(pos + 8 + len, 4);
            be_u32(&c, 0)
        };
        if (len as usize) <= CRC_CAP {
            let mut body = Vec::with_capacity(4 + len as usize);
            body.extend_from_slice(typ);
            body.extend_from_slice(&ctx.read(pos + 8, len as usize));
            let got = crc32(&body);
            if Some(got) != want_crc {
                if first {
                    return Verdict::reject(); // IHDR CRC must hold
                }
                return truncated(pos, false);
            }
        } else {
            suspect = true;
        }
        first = false;

        if typ == b"IEND" {
            let len_total = pos + chunk_total;
            let (validity, score) = if suspect {
                (Validity::Suspect, 70)
            } else {
                (Validity::Full, 97)
            };
            return Verdict::accept(len_total, validity, score);
        }
        pos = pos.saturating_add(chunk_total);
    }
    truncated(pos, first)
}

fn truncated(pos: u64, first: bool) -> Verdict {
    if first || pos < 45 {
        return Verdict::reject();
    }
    Verdict::accept(pos, Validity::Truncated, 60)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        let mut body = Vec::from(&kind[..]);
        body.extend_from_slice(data);
        v.extend_from_slice(&crc32(&body).to_be_bytes());
        v
    }

    fn synthetic_png() -> Vec<u8> {
        let mut v = Vec::from(&MAGIC[..]);
        v.extend_from_slice(&chunk(b"IHDR", &[0, 0, 0, 16, 0, 0, 0, 16, 8, 0, 0, 0, 0]));
        v.extend_from_slice(&chunk(b"IDAT", &[1, 2, 3, 4, 5, 6]));
        v.extend_from_slice(&chunk(b"rCLm", &vec![7u8; 400])); // private padding
        v.extend_from_slice(&chunk(b"IEND", &[]));
        v
    }

    fn ctx_for<'a>(r: &'a MemReader<'a>) -> Ctx<'a> {
        Ctx {
            reader: r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("image.png").unwrap(),
        }
    }

    #[test]
    fn exact_length_via_iend() {
        let png = synthetic_png();
        let end = png.len() as u64;
        let mut data = png;
        data.extend_from_slice(&[0u8; 2048]);
        let r = MemReader::new(&data);
        let v = validate(&ctx_for(&r));
        assert!(v.accept);
        assert_eq!(v.validity, Validity::Full);
        assert_eq!(v.len, end);
    }

    #[test]
    fn corrupt_crc_truncates() {
        let mut png = synthetic_png();
        // Corrupt a byte in the rCLm data (after IHDR+IDAT), flipping its CRC.
        let idx = png.len() - 20;
        png[idx] ^= 0xFF;
        let r = MemReader::new(&png);
        let v = validate(&ctx_for(&r));
        assert!(v.accept);
        assert_eq!(v.validity, Validity::Truncated);
    }
}
