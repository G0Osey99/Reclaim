//! RIFF walker (docs/plan/05 §4.7): WAV / AVI / WebP.
//!
//! `RIFF` + size (u32 LE @4) + form type @8 gives exact length `8 + size`; the
//! form type refines the id.

use super::{Ctx, Verdict};
use crate::bytes::le_u32;
use crate::Validity;

/// Validate a RIFF candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 12);
    if head.get(0..4) != Some(b"RIFF") {
        // RF64 (large WAV) also allowed.
        if head.get(0..4) == Some(b"RF64") {
            return finalize(ctx, u64::from(u32::MAX), "audio.wav", 60);
        }
        return Verdict::reject();
    }
    let size = u64::from(le_u32(&head, 4).unwrap_or(0));
    let form = head.get(8..12).unwrap_or(&[]);
    let id = match form {
        b"WAVE" => "audio.wav",
        b"AVI " => "video.avi",
        b"WEBP" => "image.webp",
        _ => return Verdict::reject(),
    };
    finalize(ctx, size.saturating_add(8), id, 88)
}

fn finalize(ctx: &Ctx, total: u64, id: &'static str, score: u8) -> Verdict {
    if total < 12 {
        return Verdict::reject();
    }
    let avail = ctx.available();
    if total <= avail {
        Verdict::accept(total, Validity::Full, score).with_format(id)
    } else {
        Verdict::accept(avail, Validity::Truncated, score.saturating_sub(35)).with_format(id)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn wav_exact_and_refined() {
        let mut v = Vec::from(&b"RIFF"[..]);
        v.extend_from_slice(&100u32.to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.resize(108, 0);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("audio.wav").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, 108);
        assert_eq!(out.format, Some("audio.wav"));
    }
}
