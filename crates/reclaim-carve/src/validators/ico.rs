//! ICO/CUR (docs/plan/05 §3.1): a weak 4-byte header, so require a plausible
//! directory (entries whose image data lies after the directory and within the
//! file) and size from the maximum entry extent.

use super::{Ctx, Verdict};
use crate::bytes::{le_u16, le_u32};
use crate::Validity;

/// Validate an ICO/CUR candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 6);
    if head.get(0..2) != Some(&[0, 0]) {
        return Verdict::reject();
    }
    match le_u16(&head, 2) {
        Some(1) | Some(2) => {}
        _ => return Verdict::reject(),
    }
    let count = le_u16(&head, 4).unwrap_or(0);
    // Icons hold a handful of images; a large count is a false positive.
    if count == 0 || count > 32 {
        return Verdict::reject();
    }
    let avail = ctx.available();
    let dir_end = 6u64 + u64::from(count) * 16;
    const MAX_ICON: u64 = 8 * 1024 * 1024;
    let mut end = dir_end;
    // Every directory entry must be plausible — a weak 4-byte magic needs a
    // strong structural check to avoid false positives (docs/plan/05 §3.1).
    for i in 0..u64::from(count) {
        let e = ctx.read(6 + i * 16, 16);
        let planes = le_u16(&e, 4).unwrap_or(0xFFFF);
        let bitcount = le_u16(&e, 6).unwrap_or(0xFFFF);
        let bytes = u64::from(le_u32(&e, 8).unwrap_or(0));
        let off = u64::from(le_u32(&e, 12).unwrap_or(0));
        let reserved = e.get(3).copied().unwrap_or(0xFF);
        let planes_ok = planes <= 1;
        let bpp_ok = matches!(bitcount, 0 | 1 | 2 | 4 | 8 | 16 | 24 | 32);
        let ext_ok = off >= dir_end && (1..=MAX_ICON).contains(&bytes) && off + bytes <= avail;
        if reserved != 0 || !planes_ok || !bpp_ok || !ext_ok {
            return Verdict::reject();
        }
        end = end.max(off + bytes);
    }
    Verdict::accept(end, Validity::Full, 72)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn rejects_random_and_accepts_valid() {
        // A valid 1-entry ICO: dir end at 22, data at 22 len 40.
        let mut v = vec![0, 0, 1, 0, 1, 0];
        v.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0]); // w,h,colors,rsvd,planes,bpp
        v.extend_from_slice(&40u32.to_le_bytes()); // bytes
        v.extend_from_slice(&22u32.to_le_bytes()); // offset
        v.resize(62, 0);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("image.ico").unwrap(),
        };
        assert!(validate(&ctx).accept);

        // Random-ish 00 00 01 00 with a bogus directory → reject.
        let bad = [0u8, 0, 1, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let r2 = MemReader::new(&bad);
        let ctx2 = Ctx {
            reader: &r2,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("image.ico").unwrap(),
        };
        assert!(!validate(&ctx2).accept);
    }
}
