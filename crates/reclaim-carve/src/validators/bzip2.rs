//! bzip2 (docs/plan/05 §3.6). Validates `BZh[1-9]` + block magic; the
//! bit-aligned end-of-stream magic makes byte-exact sizing impractical without
//! decoding, so the member is a bounded `Suspect` carve.

use super::{Ctx, Verdict};
use crate::Validity;

const CAP: u64 = 256 * 1024 * 1024;

/// Validate a bzip2 candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 10);
    if head.get(0..3) != Some(b"BZh") {
        return Verdict::reject();
    }
    if !matches!(head.get(3), Some(b'1'..=b'9')) {
        return Verdict::reject();
    }
    // Compressed-block magic 0x314159265359 or stream end 0x177245385090.
    let ok = head.get(4..10) == Some(&[0x31, 0x41, 0x59, 0x26, 0x53, 0x59])
        || head.get(4..10) == Some(&[0x17, 0x72, 0x45, 0x38, 0x50, 0x90]);
    if !ok {
        return Verdict::reject();
    }
    let len = ctx.available().min(CAP);
    if len < 14 {
        return Verdict::reject();
    }
    Verdict::accept(len, Validity::Suspect, 42).with_format("archive.bzip2")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn accepts_bzip2_header() {
        let mut v = vec![b'B', b'Z', b'h', b'9', 0x31, 0x41, 0x59, 0x26, 0x53, 0x59];
        v.extend_from_slice(&[0u8; 64]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.bzip2").unwrap(),
        };
        assert!(validate(&ctx).accept);
    }
}
