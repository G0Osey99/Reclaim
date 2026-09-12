//! MP3 (docs/plan/05 §3.4): skip an ID3v2 tag, then walk MPEG audio frames by
//! their computed length until the sync breaks; include a trailing ID3v1 `TAG`.

use super::{Ctx, Verdict};
use crate::bytes::be_u16;
use crate::metadata::Metadata;
use crate::Validity;

const MAX: u64 = 1024 * 1024 * 1024;
const MAX_FRAMES: u32 = 5_000_000;

// [version_idx][bitrate_idx] kbps. version_idx: 0=MPEG1,1=MPEG2/2.5 (Layer III).
const BITRATE: [[u16; 16]; 2] = [
    [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ],
    [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ],
];
// [version][samplerate_idx]
const SRATE: [[u32; 4]; 3] = [
    [44100, 48000, 32000, 0], // MPEG1
    [22050, 24000, 16000, 0], // MPEG2
    [11025, 12000, 8000, 0],  // MPEG2.5
];

/// Validate an MP3 candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 10);
    let mut pos: u64 = 0;
    let mut meta = Metadata::default();
    if head.get(0..3) == Some(b"ID3") {
        // ID3v2 size is 4 syncsafe bytes at offset 6.
        let s = ctx.read(6, 4);
        let size = syncsafe(&s);
        pos = 10 + size;
        meta.name = None;
    } else if head.first() != Some(&0xFF) {
        return Verdict::reject();
    }

    let cap = ctx.available().min(MAX);
    let mut frames = 0u32;
    let start_frames = pos;
    for _ in 0..MAX_FRAMES {
        let h = ctx.read(pos, 4);
        let b0 = h.first().copied().unwrap_or(0);
        let b1 = h.get(1).copied().unwrap_or(0);
        if b0 != 0xFF || (b1 & 0xE0) != 0xE0 {
            break; // sync lost
        }
        match frame_len(&h) {
            Some(len) if len >= 4 && pos + len <= cap => {
                frames += 1;
                pos += len;
            }
            _ => break,
        }
    }

    if frames < 2 {
        // Might be a lone ID3 with no audio, or garbage.
        if start_frames >= 16 && ctx.read(0, 3) == *b"ID3" {
            return Verdict::accept(start_frames.min(cap), Validity::Suspect, 40)
                .with_format("audio.mp3");
        }
        return Verdict::reject();
    }
    // Trailing ID3v1?
    let tag = ctx.read(pos, 3);
    if tag == *b"TAG" {
        pos += 128;
    }
    let validity = if pos <= cap {
        Validity::Full
    } else {
        Validity::Truncated
    };
    Verdict::accept(pos.min(cap), validity, 80)
        .with_format("audio.mp3")
        .with_meta(meta)
}

fn frame_len(h: &[u8]) -> Option<u64> {
    let b1 = *h.get(1)?;
    let b2 = *h.get(2)?;
    let version_bits = (b1 >> 3) & 0x3; // 0=2.5,1=reserved,2=2,3=1
    let layer_bits = (b1 >> 1) & 0x3; // 1=III
    if layer_bits != 0x1 {
        return None; // only Layer III
    }
    let (vtab, srtab) = match version_bits {
        3 => (0usize, 0usize), // MPEG1
        2 => (1, 1),           // MPEG2
        0 => (1, 2),           // MPEG2.5
        _ => return None,
    };
    let br_idx = (b2 >> 4) & 0xF;
    let sr_idx = (b2 >> 2) & 0x3;
    let padding = u64::from((b2 >> 1) & 0x1);
    let bitrate = u64::from(*BITRATE.get(vtab)?.get(br_idx as usize)?);
    let srate = u64::from(*SRATE.get(srtab)?.get(sr_idx as usize)?);
    if bitrate == 0 || srate == 0 {
        return None;
    }
    // Layer III: MPEG1 = 144*br/sr; MPEG2/2.5 = 72*br/sr (samples per frame /8).
    let coeff = if version_bits == 3 { 144_000 } else { 72_000 };
    let _ = be_u16(h, 0);
    Some(coeff * bitrate / srate + padding)
}

fn syncsafe(b: &[u8]) -> u64 {
    let v = |i: usize| u64::from(b.get(i).copied().unwrap_or(0) & 0x7F);
    (v(0) << 21) | (v(1) << 14) | (v(2) << 7) | v(3)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    // MPEG1 Layer III, 128 kbps, 44100 Hz, no padding → 417 bytes/frame.
    fn frame() -> Vec<u8> {
        let mut f = vec![0xFF, 0xFB, 0x90, 0x00]; // sync, MPEG1 L3, 128k/44.1k
        f.resize(417, 0);
        f
    }

    #[test]
    fn walks_frames() {
        let mut v = Vec::new();
        for _ in 0..4 {
            v.extend_from_slice(&frame());
        }
        let end = v.len() as u64;
        v.extend_from_slice(&[0u8; 32]); // silence breaks sync
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("audio.mp3").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept, "should accept 4 frames");
        assert_eq!(out.len, end);
    }
}
