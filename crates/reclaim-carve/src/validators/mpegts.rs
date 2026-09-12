//! MPEG-TS (docs/plan/05 §3.3): 188-byte (or 192-byte M2TS) packets synced on
//! `0x47`. Requires ≥5 consecutive in-sync packets, then counts packets to the
//! end of the synced run.
//!
//! Note: the single-byte `0x47` anchor is excluded from the Aho-Corasick
//! matcher (it would match ~1/256 of all bytes), so this validator is reached
//! only via a dedicated periodic-sync pass (a Phase-1 gap, see phase-1.md) or
//! `sigs test`. It is kept correct and fuzzed regardless.

use super::{Ctx, Verdict};
use crate::Validity;

const MAX: u64 = 16 * 1024 * 1024 * 1024;

/// Validate an MPEG-TS candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let probe = ctx.read(0, 188 * 6);
    let stride = if probe.first() == Some(&0x47) {
        188
    } else if probe.get(4) == Some(&0x47) {
        192 // M2TS with 4-byte TP_extra header
    } else {
        return Verdict::reject();
    };
    let sync_off = if stride == 192 { 4 } else { 0 };
    // Require >= 5 consecutive syncs.
    for i in 0..5 {
        if probe.get(sync_off + i * stride).copied() != Some(0x47) {
            return Verdict::reject();
        }
    }
    let cap = ctx.available().min(MAX);
    let mut pos: u64 = 0;
    let mut packets = 0u64;
    for _ in 0..50_000_000u64 {
        let b = ctx.read(pos + sync_off as u64, 1);
        if b.first().copied() != Some(0x47) {
            break;
        }
        packets += 1;
        pos += stride as u64;
        if pos > cap {
            pos = cap;
            break;
        }
    }
    if packets < 5 {
        return Verdict::reject();
    }
    Verdict::accept(pos.min(cap), Validity::Full, 80).with_format("video.mpeg_ts")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn counts_packets() {
        let mut v = Vec::new();
        for _ in 0..10 {
            v.push(0x47);
            v.extend_from_slice(&[0u8; 187]);
        }
        let end = v.len() as u64;
        v.extend_from_slice(&[0u8; 188]); // no sync → stop
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("video.mpeg_ts").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, end);
    }
}
