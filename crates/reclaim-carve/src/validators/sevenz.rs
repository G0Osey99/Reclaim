//! 7-Zip (docs/plan/05 §3.6): exact size from the start header — the next-header
//! offset + size (both u64 LE) after the 32-byte signature header.

use super::{Ctx, Verdict};
use crate::bytes::le_u64;
use crate::Validity;

const MAGIC: &[u8] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];

/// Validate a 7z candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 32);
    if head.get(0..6) != Some(MAGIC) {
        return Verdict::reject();
    }
    // Signature header: 6 magic + 2 version + 4 startHeaderCRC + 20 start header.
    let next_off = le_u64(&head, 12).unwrap_or(0);
    let next_size = le_u64(&head, 20).unwrap_or(0);
    let total = 32u64.saturating_add(next_off).saturating_add(next_size);
    if total < 32 {
        return Verdict::reject();
    }
    let avail = ctx.available();
    if total <= avail {
        Verdict::accept(total, Validity::Full, 90)
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
    fn exact_from_start_header() {
        let mut v = Vec::from(MAGIC);
        v.extend_from_slice(&[0, 4]); // version
        v.extend_from_slice(&[0u8; 4]); // crc
        v.extend_from_slice(&100u64.to_le_bytes()); // next header offset
        v.extend_from_slice(&20u64.to_le_bytes()); // next header size
        v.resize(32 + 100 + 20, 0);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.7z").unwrap(),
        };
        assert_eq!(validate(&ctx).len, 32 + 100 + 20);
    }
}
