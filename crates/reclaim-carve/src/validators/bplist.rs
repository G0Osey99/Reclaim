//! Apple binary plist (docs/plan/05 §3.5). `bplist00` + a 32-byte trailer at
//! EOF. The trailer's `offsetTableOffset` sizes the file, but it lives at the
//! (unknown) end, so forward carving emits a bounded `Suspect` result.

use super::{Ctx, Verdict};
use crate::Validity;

const CAP: u64 = 64 * 1024 * 1024;

/// Validate a bplist candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    if ctx.read(0, 8) != *b"bplist00" {
        return Verdict::reject();
    }
    let len = ctx.available().min(CAP).min(4 * 1024 * 1024);
    if len < 40 {
        return Verdict::reject();
    }
    Verdict::accept(len, Validity::Suspect, 45).with_format("doc.bplist")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn accepts_header() {
        let mut v = Vec::from(&b"bplist00"[..]);
        v.extend_from_slice(&[0u8; 64]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("doc.bplist").unwrap(),
        };
        assert!(validate(&ctx).accept);
    }
}
