//! FLAC (docs/plan/05 §3.4). Validates `fLaC` + the metadata-block chain
//! (STREAMINFO first, last-block flag terminating); audio-frame sizing needs
//! decoding, so the stream is a bounded `Suspect` carve past the metadata.

use super::{Ctx, Verdict};
use crate::bytes::be_u32;
use crate::Validity;

const CAP: u64 = 2 * 1024 * 1024 * 1024;

/// Validate a FLAC candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    if ctx.read(0, 4) != *b"fLaC" {
        return Verdict::reject();
    }
    let cap = ctx.available().min(CAP);
    let mut pos: u64 = 4;
    let mut first = true;
    for _ in 0..4096 {
        let hdr = ctx.read(pos, 4);
        let b0 = match hdr.first() {
            Some(b) => *b,
            None => break,
        };
        let last = b0 & 0x80 != 0;
        let block_type = b0 & 0x7F;
        if first && block_type != 0 {
            // First metadata block must be STREAMINFO (type 0).
            return Verdict::reject();
        }
        first = false;
        let size = u64::from(
            be_u32(
                &[
                    0,
                    hdr.get(1).copied().unwrap_or(0),
                    hdr.get(2).copied().unwrap_or(0),
                    hdr.get(3).copied().unwrap_or(0),
                ],
                0,
            )
            .unwrap_or(0),
        );
        pos += 4 + size;
        if pos > cap {
            return Verdict::reject();
        }
        if last {
            break;
        }
    }
    if pos < 42 {
        return Verdict::reject();
    }
    // Audio frames follow; bounded Suspect carve to end of source region.
    Verdict::accept(cap, Validity::Suspect, 55).with_format("audio.flac")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn parses_metadata_chain() {
        let mut v = Vec::from(&b"fLaC"[..]);
        // STREAMINFO, last block, size 34
        v.push(0x80);
        v.extend_from_slice(&[0, 0, 34]);
        v.extend_from_slice(&[0u8; 34]);
        v.extend_from_slice(&[0xAAu8; 200]); // "audio"
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("audio.flac").unwrap(),
        };
        assert!(validate(&ctx).accept);
    }
}
