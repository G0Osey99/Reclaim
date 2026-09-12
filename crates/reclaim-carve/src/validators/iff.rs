//! IFF/AIFF walker (docs/plan/05 §4.7): `FORM` + size (u32 BE @4) + form type
//! (`AIFF`/`AIFC`) → exact length `8 + size`.

use super::{Ctx, Verdict};
use crate::bytes::be_u32;
use crate::Validity;

/// Validate an AIFF/AIFC candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 12);
    if head.get(0..4) != Some(b"FORM") {
        return Verdict::reject();
    }
    let form = head.get(8..12).unwrap_or(&[]);
    if form != b"AIFF" && form != b"AIFC" {
        return Verdict::reject();
    }
    let size = u64::from(be_u32(&head, 4).unwrap_or(0));
    let total = size.saturating_add(8);
    if total < 12 {
        return Verdict::reject();
    }
    let avail = ctx.available();
    if total <= avail {
        Verdict::accept(total, Validity::Full, 88).with_format("audio.aiff")
    } else {
        Verdict::accept(avail, Validity::Truncated, 50).with_format("audio.aiff")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn aiff_exact() {
        let mut v = Vec::from(&b"FORM"[..]);
        v.extend_from_slice(&40u32.to_be_bytes());
        v.extend_from_slice(b"AIFF");
        v.resize(48, 0);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("audio.aiff").unwrap(),
        };
        assert_eq!(validate(&ctx).len, 48);
    }
}
