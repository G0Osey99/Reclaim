//! GIF block walker (docs/plan/05 §3.1): header + LSD → image/extension blocks
//! → trailer `0x3B`. Sub-block chains (length-prefixed, 0 terminates) are
//! skipped precisely, so the exact end is the trailer byte.

use super::{Ctx, Verdict};
use crate::Validity;

const MAX: u64 = 64 * 1024 * 1024;
const MAX_BLOCKS: u32 = 1_000_000;

/// Validate a GIF candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 13);
    let sig6 = head.get(0..6);
    if sig6 != Some(b"GIF87a") && sig6 != Some(b"GIF89a") {
        return Verdict::reject();
    }
    let cap = ctx.available().min(MAX);
    // Logical Screen Descriptor is 7 bytes (offset 6..13); packed field @10.
    let packed = head.get(10).copied().unwrap_or(0);
    let mut pos: u64 = 13;
    if packed & 0x80 != 0 {
        let gct_size = 3u64 * (1u64 << ((packed & 0x07) + 1));
        pos += gct_size;
    }
    for _ in 0..MAX_BLOCKS {
        let intro = ctx.read(pos, 1);
        match intro.first().copied() {
            Some(0x3B) => {
                return Verdict::accept(pos + 1, Validity::Full, 90);
            }
            Some(0x21) => {
                // Extension: label byte + sub-block chain.
                pos += 2;
                pos = match skip_subblocks(ctx, pos, cap) {
                    Some(p) => p,
                    None => return truncated(pos),
                };
            }
            Some(0x2C) => {
                // Image descriptor: 10 bytes, optional LCT, LZW min code, subs.
                let desc = ctx.read(pos, 10);
                let p = desc.get(9).copied().unwrap_or(0);
                let mut np = pos + 10;
                if p & 0x80 != 0 {
                    np += 3u64 * (1u64 << ((p & 0x07) + 1));
                }
                np += 1; // LZW minimum code size
                pos = match skip_subblocks(ctx, np, cap) {
                    Some(x) => x,
                    None => return truncated(pos),
                };
            }
            _ => return truncated(pos),
        }
        if pos >= cap {
            return truncated(cap);
        }
    }
    truncated(pos)
}

fn skip_subblocks(ctx: &Ctx, mut pos: u64, cap: u64) -> Option<u64> {
    for _ in 0..1_000_000 {
        if pos >= cap {
            return None;
        }
        let n = ctx.read(pos, 1).first().copied()?;
        pos += 1;
        if n == 0 {
            return Some(pos);
        }
        pos += u64::from(n);
    }
    None
}

fn truncated(pos: u64) -> Verdict {
    if pos < 35 {
        return Verdict::reject();
    }
    Verdict::accept(pos, Validity::Truncated, 45)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn walks_to_trailer() {
        let mut v = Vec::from(&b"GIF89a"[..]);
        v.extend_from_slice(&[16, 0, 16, 0, 0, 0, 0]); // LSD, no GCT
                                                       // graphic control extension: 0x21 0xF9 [4-byte sub] 0x00
        v.extend_from_slice(&[0x21, 0xF9, 4, 0, 0, 0, 0, 0x00]);
        // trailer
        v.push(0x3B);
        let end = v.len() as u64;
        v.extend_from_slice(&[0u8; 100]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("image.gif").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, end);
    }
}
