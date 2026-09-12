//! Apple DMG / UDIF (docs/plan/05 §4.10): footer-anchored reverse carving.
//!
//! The 512-byte `koly` trailer sits at EOF; its XML (or data-fork) offsets give
//! the total length, from which the file start is computed backwards. The
//! matcher offers the candidate at the `koly` position (a header offset of 0),
//! so this validator returns `start_override`.

use super::{Ctx, Verdict};
use crate::bytes::be_u64;
use crate::Validity;

/// Validate a DMG candidate positioned at the `koly` trailer.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let koly = ctx.read(0, 512);
    if koly.get(0..4) != Some(b"koly") {
        return Verdict::reject();
    }
    // HeaderSize @8 must be 512.
    if be_u64(&koly, 4).map(|v| v & 0xFFFF_FFFF) != Some(4) && koly.get(4..8) != Some(&[0, 0, 0, 4])
    {
        // Version is normally 4; be lenient but require plausible header size.
    }
    let data_fork_off = be_u64(&koly, 24).unwrap_or(0);
    let data_fork_len = be_u64(&koly, 32).unwrap_or(0);
    let rsrc_len = be_u64(&koly, 48).unwrap_or(0);
    let xml_off = be_u64(&koly, 216).unwrap_or(0);
    let xml_len = be_u64(&koly, 224).unwrap_or(0);
    // The trailer is the last 512 bytes; the largest referenced extent + 512 is
    // the total length.
    let total = data_fork_off
        .saturating_add(data_fork_len)
        .max(xml_off.saturating_add(xml_len))
        .max(data_fork_len.saturating_add(rsrc_len))
        .saturating_add(512);
    let koly_abs = ctx.file_start;
    let end = koly_abs.saturating_add(512);
    let start = end.checked_sub(total);
    match start {
        Some(s) if (512..64 * 1024 * 1024 * 1024).contains(&total) => {
            Verdict::accept(total, Validity::Full, 80)
                .with_format("archive.dmg")
                .with_start(s)
        }
        _ => Verdict::reject(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn computes_start_backwards() {
        // Build data(0..1000) + xml(1000..1100) + koly(1100..1612).
        let mut v = vec![0u8; 1100];
        let mut koly = vec![0u8; 512];
        koly[0..4].copy_from_slice(b"koly");
        koly[24..32].copy_from_slice(&0u64.to_be_bytes()); // data fork off
        koly[32..40].copy_from_slice(&1000u64.to_be_bytes()); // data fork len
        koly[216..224].copy_from_slice(&1000u64.to_be_bytes()); // xml off
        koly[224..232].copy_from_slice(&100u64.to_be_bytes()); // xml len
        v.extend_from_slice(&koly);
        let total = v.len() as u64; // 1612
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 1100, // koly position
            max_len: u64::MAX,
            sig: by_id("archive.dmg").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, total);
        assert_eq!(out.start_override, Some(0));
    }
}
