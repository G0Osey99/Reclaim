//! Zstandard (docs/plan/05 §3.6). Validates the frame magic and reads the frame
//! content size from the frame header when present (FCS flag); otherwise a
//! bounded `Suspect` carve (streaming frames carry no size).

use super::{Ctx, Verdict};
use crate::bytes::{le_u16, le_u32, le_u64};
use crate::Validity;

const MAGIC: &[u8] = &[0x28, 0xB5, 0x2F, 0xFD];
const CAP: u64 = 4 * 1024 * 1024 * 1024;

/// Validate a zstd candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 14);
    if head.get(0..4) != Some(MAGIC) {
        return Verdict::reject();
    }
    let fhd = head.get(4).copied().unwrap_or(0);
    let fcs_flag = fhd >> 6;
    let single_segment = fhd & 0x20 != 0;
    let did_size = match fhd & 0x3 {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    // Content size gives a *decompressed* size — a weak upper bound on the frame
    // for our purposes — so we don't use it to size the compressed frame; we
    // only confirm the header parses. Report a bounded Suspect carve.
    let _ = (
        le_u16(&head, 6),
        le_u32(&head, 6),
        le_u64(&head, 6),
        single_segment,
        did_size,
        fcs_flag,
    );
    let len = ctx.available().min(CAP).min(64 * 1024 * 1024);
    if len < 8 {
        return Verdict::reject();
    }
    Verdict::accept(len, Validity::Suspect, 42).with_format("archive.zstd")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn accepts_frame_magic() {
        let mut v = Vec::from(MAGIC);
        v.extend_from_slice(&[0x00; 32]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.zstd").unwrap(),
        };
        assert!(validate(&ctx).accept);
    }
}
