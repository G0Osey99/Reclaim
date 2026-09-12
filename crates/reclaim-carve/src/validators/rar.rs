//! RAR (docs/plan/05 §3.6). Validates the v4 (`Rar!\x1a\x07\x00`) or v5
//! (`Rar!\x1a\x07\x01\x00`) signature; block-accurate sizing across encrypted
//! headers is deferred, so the archive is a bounded `Suspect` carve.

use super::{Ctx, Verdict};
use crate::Validity;

const V4: &[u8] = &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const V5: &[u8] = &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];
const CAP: u64 = 16 * 1024 * 1024 * 1024;

/// Validate a RAR candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 8);
    let is_v5 = head.get(0..8) == Some(V5);
    let is_v4 = head.get(0..7) == Some(V4);
    if !is_v4 && !is_v5 {
        return Verdict::reject();
    }
    let len = ctx.available().min(CAP).min(256 * 1024 * 1024);
    if len < 20 {
        return Verdict::reject();
    }
    Verdict::accept(len, Validity::Suspect, 45).with_format("archive.rar")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn accepts_v5() {
        let mut v = Vec::from(V5);
        v.extend_from_slice(&[0u8; 64]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.rar").unwrap(),
        };
        assert!(validate(&ctx).accept);
    }
}
