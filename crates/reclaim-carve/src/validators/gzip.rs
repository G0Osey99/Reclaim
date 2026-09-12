//! GZIP (docs/plan/05 §3.6). Validates the 10-byte header and extracts the
//! original filename (FNAME flag) for naming. Exact end requires inflating the
//! deflate stream, which round 1 does not do: the contiguous member is carved
//! as a bounded `Suspect` (documented in phase-1.md).

use super::{Ctx, Verdict};
use crate::bytes::le_u16;
use crate::metadata::Metadata;
use crate::Validity;

const CAP: u64 = 256 * 1024 * 1024;

/// Validate a GZIP candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 10);
    if head.get(0..3) != Some(&[0x1F, 0x8B, 0x08]) {
        return Verdict::reject();
    }
    let flg = head.get(3).copied().unwrap_or(0);
    // MTIME(4)+XFL(1)+OS(1) already in the 10 header bytes. Optional fields:
    let mut pos: u64 = 10;
    if flg & 0x04 != 0 {
        // FEXTRA
        let xlen = u64::from(le_u16(&ctx.read(pos, 2), 0).unwrap_or(0));
        pos += 2 + xlen;
    }
    let mut meta = Metadata::default();
    if flg & 0x08 != 0 {
        // FNAME: null-terminated original name.
        let raw = ctx.read(pos, 256);
        let name: String = raw
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as char)
            .collect();
        if !name.is_empty() {
            meta.name = Some(name);
        }
    }
    let len = ctx.available().min(CAP);
    if len < 18 {
        return Verdict::reject();
    }
    Verdict::accept(len, Validity::Suspect, 42)
        .with_format("archive.gzip")
        .with_meta(meta)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn extracts_fname() {
        let mut v = vec![0x1F, 0x8B, 0x08, 0x08, 0, 0, 0, 0, 0, 3];
        v.extend_from_slice(b"orig.tar\0");
        v.extend_from_slice(&[0u8; 100]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.gzip").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.meta.name.as_deref(), Some("orig.tar"));
    }
}
