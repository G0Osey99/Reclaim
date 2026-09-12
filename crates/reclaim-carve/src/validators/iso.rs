//! ISO 9660 (docs/plan/05 §3.6): the Primary Volume Descriptor at LBA 16 gives
//! `volume_space_size × logical_block_size` = exact image length.
//!
//! The `CD001` magic sits at offset 32769, so `ctx.file_start` is the image
//! start and the PVD begins at +32768.

use super::{Ctx, Verdict};
use crate::bytes::{le_u16, le_u32};
use crate::Validity;

const PVD: u64 = 32768;

/// Validate an ISO candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let desc = ctx.read(PVD, 256);
    // type byte 1 (primary), "CD001", version 1.
    if desc.first() != Some(&1) || desc.get(1..6) != Some(b"CD001") {
        return Verdict::reject();
    }
    // Volume space size: both-endian u32 at descriptor offset 80 (LSB first).
    let vss = u64::from(le_u32(&desc, 80).unwrap_or(0));
    // Logical block size: both-endian u16 at offset 128 (LSB first).
    let lbs = u64::from(le_u16(&desc, 128).unwrap_or(0));
    if vss == 0 || lbs == 0 || !lbs.is_power_of_two() {
        return Verdict::reject();
    }
    let total = vss.saturating_mul(lbs);
    if total < PVD {
        return Verdict::reject();
    }
    let avail = ctx.available();
    if total <= avail {
        Verdict::accept(total, Validity::Full, 88).with_format("archive.iso")
    } else {
        Verdict::accept(avail, Validity::Truncated, 50).with_format("archive.iso")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn size_from_pvd() {
        let mut v = vec![0u8; PVD as usize + 256];
        v[PVD as usize] = 1;
        v[PVD as usize + 1..PVD as usize + 6].copy_from_slice(b"CD001");
        v[PVD as usize + 80..PVD as usize + 84].copy_from_slice(&40u32.to_le_bytes()); // 40 blocks
        v[PVD as usize + 128..PVD as usize + 130].copy_from_slice(&2048u16.to_le_bytes());
        // grow to the computed size
        v.resize(40 * 2048, 0);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.iso").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, 40 * 2048);
    }
}
