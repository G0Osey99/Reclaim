//! OLE2 / Compound File Binary (docs/plan/05 §4.5): legacy Office / Outlook
//! `.msg`. Validates the header and estimates the size from the FAT sector
//! count (an upper bound; a full FAT walk for the exact tail is deferred).

use super::{Ctx, Verdict};
use crate::bytes::{le_u16, le_u32};
use crate::Validity;

const MAGIC: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Validate an OLE2 candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 512);
    if head.get(0..8) != Some(MAGIC) {
        return Verdict::reject();
    }
    let sector_shift = le_u16(&head, 30).unwrap_or(0);
    if !(7..=12).contains(&sector_shift) {
        return Verdict::reject();
    }
    let sector_size = 1u64 << sector_shift;
    let num_fat = u64::from(le_u32(&head, 44).unwrap_or(0));
    if num_fat == 0 || num_fat > 1_000_000 {
        return Verdict::reject();
    }
    // Each FAT sector describes sector_size/4 sectors; upper bound on the file.
    let sectors_per_fat = sector_size / 4;
    let max_sectors = num_fat.saturating_mul(sectors_per_fat);
    let upper = sector_size.saturating_add(max_sectors.saturating_mul(sector_size));
    let avail = ctx.available();
    let len = upper.min(avail);
    if len < 512 {
        return Verdict::reject();
    }
    // Upper-bound sizing → Suspect (may over-read free sectors).
    Verdict::accept(len, Validity::Suspect, 55).with_format("doc.ole2")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn accepts_and_bounds() {
        let mut v = vec![0u8; 4096];
        v[0..8].copy_from_slice(MAGIC);
        v[30..32].copy_from_slice(&9u16.to_le_bytes()); // 512-byte sectors
        v[44..48].copy_from_slice(&1u32.to_le_bytes()); // 1 FAT sector
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("doc.ole2").unwrap(),
        };
        assert!(validate(&ctx).accept);
    }
}
