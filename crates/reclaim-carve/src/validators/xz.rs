//! XZ (docs/plan/05 §3.6). Validates the 6-byte stream header + flags; scans
//! forward for the stream footer magic `YZ` to size the stream when found,
//! otherwise a bounded `Suspect` carve.

use super::{Ctx, Verdict};
use crate::Validity;

const HEADER: &[u8] = &[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];
const CAP: u64 = 512 * 1024 * 1024;
const WIN: usize = 1 << 20;

/// Validate an XZ candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    if ctx.read(0, 6) != *HEADER {
        return Verdict::reject();
    }
    let cap = ctx.available().min(CAP);
    // The stream footer ends with `YZ`; its 4-aligned position bounds the size.
    let mut pos: u64 = 12;
    let mut last: Option<u64> = None;
    while pos < cap {
        let want = (cap - pos).min(WIN as u64) as usize;
        let buf = ctx.read(pos, want);
        if buf.is_empty() {
            break;
        }
        for m in memchr::memmem::find_iter(&buf, b"YZ") {
            last = Some(pos + m as u64 + 2);
        }
        if want < WIN {
            break;
        }
        pos += (want - 1) as u64;
    }
    match last {
        Some(end) => Verdict::accept(end.min(cap), Validity::Full, 70).with_format("archive.xz"),
        None => Verdict::accept(cap.min(64 * 1024 * 1024), Validity::Suspect, 42)
            .with_format("archive.xz"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn finds_footer() {
        let mut v = Vec::from(HEADER);
        v.extend_from_slice(&[0u8; 40]);
        v.extend_from_slice(b"YZ");
        let end = v.len() as u64;
        v.extend_from_slice(&[0u8; 100]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.xz").unwrap(),
        };
        assert_eq!(validate(&ctx).len, end);
    }
}
