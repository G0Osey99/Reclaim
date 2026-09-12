//! TAR (docs/plan/05 §3.6): 512-byte record walk with header-checksum
//! validation; two consecutive zero blocks mark the end (exact size).

use super::{Ctx, Verdict};
use crate::Validity;

const MAX: u64 = 16 * 1024 * 1024 * 1024;
const MAX_RECORDS: u32 = 20_000_000;

/// Validate a TAR candidate. The `ustar` magic sits at offset 257, so the
/// candidate `file_start` is already the record start.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let first = ctx.read(0, 512);
    if !checksum_ok(&first) {
        return Verdict::reject();
    }
    let cap = ctx.available().min(MAX);
    let mut pos: u64 = 0;
    let mut zero_blocks = 0u32;
    for _ in 0..MAX_RECORDS {
        if pos + 512 > cap {
            return Verdict::accept(cap, Validity::Truncated, 45).with_format("archive.tar");
        }
        let hdr = ctx.read(pos, 512);
        if hdr.iter().all(|b| *b == 0) {
            zero_blocks += 1;
            pos += 512;
            if zero_blocks >= 2 {
                return Verdict::accept(pos, Validity::Full, 88).with_format("archive.tar");
            }
            continue;
        }
        zero_blocks = 0;
        if !checksum_ok(&hdr) {
            // End of a tar with no zero terminator, or corruption.
            if pos >= 512 {
                return Verdict::accept(pos, Validity::Full, 70).with_format("archive.tar");
            }
            return Verdict::reject();
        }
        let size = octal(&hdr, 124, 12);
        let data_blocks = size.div_ceil(512);
        pos += 512 + data_blocks * 512;
    }
    Verdict::accept(pos.min(cap), Validity::Truncated, 50).with_format("archive.tar")
}

fn checksum_ok(hdr: &[u8]) -> bool {
    if hdr.len() < 512 {
        return false;
    }
    let stored = octal(hdr, 148, 8);
    let mut sum: u64 = 0;
    for (i, b) in hdr.iter().take(512).enumerate() {
        // The checksum field itself is treated as spaces.
        if (148..156).contains(&i) {
            sum += 32;
        } else {
            sum += u64::from(*b);
        }
    }
    stored == sum
}

fn octal(b: &[u8], off: usize, len: usize) -> u64 {
    let mut v: u64 = 0;
    for i in 0..len {
        match b.get(off + i) {
            Some(c) if (b'0'..=b'7').contains(c) => v = v * 8 + u64::from(c - b'0'),
            _ => break,
        }
    }
    v
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    fn tar_header(name: &str, size: u64) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        let oct = format!("{size:011o}\0");
        h[124..124 + oct.len()].copy_from_slice(oct.as_bytes());
        h[257..262].copy_from_slice(b"ustar");
        // checksum
        let mut sum: u64 = 0;
        for (i, b) in h.iter().enumerate() {
            sum += if (148..156).contains(&i) {
                32
            } else {
                u64::from(*b)
            };
        }
        let cs = format!("{sum:06o}\0 ");
        h[148..148 + cs.len()].copy_from_slice(cs.as_bytes());
        h
    }

    #[test]
    fn walks_records_to_zero_terminator() {
        let mut v = tar_header("a.txt", 10);
        v.extend_from_slice(&[0u8; 512]); // data block (10 bytes rounded to 512)
        v.extend_from_slice(&[0u8; 512]); // zero block 1
        v.extend_from_slice(&[0u8; 512]); // zero block 2 → end
        let end = v.len() as u64;
        v.extend_from_slice(&[0xAA; 512]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.tar").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, end);
    }
}
