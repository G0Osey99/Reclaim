//! Windows BMP (docs/plan/05 §3.1): exact size from the file-size field.
//!
//! `BM` + total file size (u32 LE @2). Sanity: pixel-data offset (@10) and DIB
//! header size (@14) must be plausible.

use super::{Ctx, Verdict};
use crate::bytes::le_u32;
use crate::Validity;

/// Validate a BMP candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 26);
    if head.first() != Some(&0x42) || head.get(1) != Some(&0x4D) {
        return Verdict::reject();
    }
    let size = u64::from(le_u32(&head, 2).unwrap_or(0));
    let data_off = u64::from(le_u32(&head, 10).unwrap_or(0));
    let dib = le_u32(&head, 14).unwrap_or(0);
    // Known DIB header sizes: 12 (core), 40 (info), 108 (v4), 124 (v5), 52/56/64.
    let dib_ok = matches!(dib, 12 | 40 | 52 | 56 | 64 | 108 | 124);
    if !dib_ok || size < 26 || data_off < 26 || data_off > size {
        return Verdict::reject();
    }
    let avail = ctx.available();
    if size <= avail {
        Verdict::accept(size, Validity::Full, 88)
    } else {
        Verdict::accept(avail, Validity::Truncated, 50)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn size_from_field() {
        let mut b = vec![0u8; 200];
        b[0] = 0x42;
        b[1] = 0x4D;
        b[2..6].copy_from_slice(&130u32.to_le_bytes());
        b[10..14].copy_from_slice(&54u32.to_le_bytes());
        b[14..18].copy_from_slice(&40u32.to_le_bytes());
        let r = MemReader::new(&b);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("image.bmp").unwrap(),
        };
        let v = validate(&ctx);
        assert!(v.accept);
        assert_eq!(v.len, 130);
    }
}
