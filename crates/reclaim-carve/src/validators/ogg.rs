//! Ogg page walker (docs/plan/05 §3.4): `OggS` pages summed to the end-of-stream
//! page; first-page codec id refines Vorbis/Opus/Theora/FLAC.

use super::{Ctx, Verdict};
use crate::Validity;

const MAX: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PAGES: u32 = 5_000_000;

/// Validate an Ogg candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    if ctx.read(0, 4) != *b"OggS" {
        return Verdict::reject();
    }
    let cap = ctx.available().min(MAX);
    let mut pos: u64 = 0;
    let mut id = "audio.ogg";
    let mut first = true;
    for _ in 0..MAX_PAGES {
        let hdr = ctx.read(pos, 27);
        if hdr.get(0..4) != Some(b"OggS") {
            break;
        }
        let header_type = hdr.get(5).copied().unwrap_or(0);
        let nsegs = hdr.get(26).copied().unwrap_or(0) as usize;
        let segtab = ctx.read(pos + 27, nsegs);
        let body: u64 = segtab.iter().map(|b| u64::from(*b)).sum();
        let page_len = 27u64 + nsegs as u64 + body;
        if first {
            let payload = ctx.read(pos + 27 + nsegs as u64, 8);
            id = refine(&payload);
            first = false;
        }
        let end = pos + page_len;
        if end > cap {
            return Verdict::accept(cap, Validity::Truncated, 45).with_format(id);
        }
        pos = end;
        if header_type & 0x04 != 0 {
            // End-of-stream page.
            return Verdict::accept(pos, Validity::Full, 88).with_format(id);
        }
    }
    if pos < 58 {
        return Verdict::reject();
    }
    Verdict::accept(pos.min(cap), Validity::Truncated, 55).with_format(id)
}

fn refine(payload: &[u8]) -> &'static str {
    if payload.starts_with(b"OpusHead") || payload.get(1..7) == Some(b"vorbis") {
        "audio.ogg"
    } else if payload.get(1..7) == Some(b"theora") {
        "video.ogv"
    } else {
        "audio.ogg"
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    fn page(header_type: u8, body: &[u8]) -> Vec<u8> {
        let mut v = Vec::from(&b"OggS"[..]);
        v.push(0); // version
        v.push(header_type);
        v.extend_from_slice(&[0u8; 8]); // granule
        v.extend_from_slice(&[0u8; 4]); // serial
        v.extend_from_slice(&[0u8; 4]); // seq
        v.extend_from_slice(&[0u8; 4]); // crc
        v.push(1); // 1 segment
        v.push(body.len() as u8);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn walks_to_eos() {
        let mut v = page(0x02, b"OpusHead");
        v.extend_from_slice(&page(0x04, &[1, 2, 3])); // EOS page
        let end = v.len() as u64;
        v.extend_from_slice(&[0u8; 64]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("audio.ogg").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, end);
    }
}
