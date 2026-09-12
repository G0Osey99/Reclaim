//! SQLite database file (docs/plan/05 §4.8): exact size from header fields.
//!
//! `"SQLite format 3\0"` + page size (u16 @16, value 1 ⇒ 65536) × page count
//! (u32 @28) = exact byte length. Page-1 sanity keeps false positives down.

use super::{Ctx, Verdict};
use crate::bytes::{be_u16, be_u32};
use crate::Validity;

const MAGIC: &[u8] = b"SQLite format 3\x00";

/// Validate a SQLite candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 100);
    if head.get(..16) != Some(MAGIC) {
        return Verdict::reject();
    }
    let raw_ps = be_u16(&head, 16).unwrap_or(0);
    let page_size: u64 = if raw_ps == 1 {
        65536
    } else {
        u64::from(raw_ps)
    };
    if page_size < 512 || !page_size.is_power_of_two() {
        return Verdict::reject();
    }
    let page_count = u64::from(be_u32(&head, 28).unwrap_or(0));
    if page_count == 0 {
        return Verdict::reject();
    }
    // Page 1 encoding sanity: read/write versions ∈ {1,2}.
    let rv = head.get(18).copied().unwrap_or(9);
    let wv = head.get(19).copied().unwrap_or(9);
    if rv > 2 || wv > 2 {
        return Verdict::reject();
    }
    let size = page_size.saturating_mul(page_count);
    let avail = ctx.available();
    if size <= avail {
        Verdict::accept(size, Validity::Full, 92)
    } else {
        Verdict::accept(avail, Validity::Truncated, 55)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn exact_size_from_header() {
        let mut h = vec![0u8; 4096 * 3];
        h[..16].copy_from_slice(MAGIC);
        h[16..18].copy_from_slice(&4096u16.to_be_bytes());
        h[18] = 1;
        h[19] = 1;
        h[28..32].copy_from_slice(&3u32.to_be_bytes()); // 3 pages
        let r = MemReader::new(&h);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("db.sqlite").unwrap(),
        };
        let v = validate(&ctx);
        assert!(v.accept);
        assert_eq!(v.len, 4096 * 3);
        assert_eq!(v.validity, Validity::Full);
    }
}
