//! PE / EXE / DLL (docs/plan/05 §4): `MZ` → `PE\0\0` at `e_lfanew`; size from
//! the maximum section `PointerToRawData + SizeOfRawData`.

use super::{Ctx, Verdict};
use crate::bytes::{le_u16, le_u32};
use crate::Validity;

/// Validate a PE candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let dos = ctx.read(0, 64);
    if dos.first() != Some(&b'M') || dos.get(1) != Some(&b'Z') {
        return Verdict::reject();
    }
    let e_lfanew = u64::from(le_u32(&dos, 0x3C).unwrap_or(0));
    if !(64..=4096).contains(&e_lfanew) {
        return Verdict::reject();
    }
    let coff = ctx.read(e_lfanew, 24);
    if coff.get(0..4) != Some(&[0x50, 0x45, 0x00, 0x00]) {
        return Verdict::reject();
    }
    let num_sections = le_u16(&coff, 6).unwrap_or(0) as u64;
    let opt_hdr_size = le_u16(&coff, 20).unwrap_or(0) as u64;
    if num_sections == 0 || num_sections > 96 {
        return Verdict::reject();
    }
    let sect_start = e_lfanew + 24 + opt_hdr_size;
    let mut end = sect_start + num_sections * 40;
    for i in 0..num_sections {
        let sh = ctx.read(sect_start + i * 40, 40);
        let size_raw = u64::from(le_u32(&sh, 16).unwrap_or(0));
        let ptr_raw = u64::from(le_u32(&sh, 20).unwrap_or(0));
        end = end.max(ptr_raw.saturating_add(size_raw));
    }
    let avail = ctx.available();
    if end < 97 {
        return Verdict::reject();
    }
    if end <= avail {
        Verdict::accept(end, Validity::Full, 85).with_format("exec.pe")
    } else {
        Verdict::accept(avail, Validity::Truncated, 50).with_format("exec.pe")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn section_extent() {
        let mut v = vec![0u8; 8192];
        v[0] = b'M';
        v[1] = b'Z';
        let e_lfanew = 128u32;
        v[0x3C..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
        let p = e_lfanew as usize;
        v[p..p + 4].copy_from_slice(&[0x50, 0x45, 0x00, 0x00]);
        v[p + 6..p + 8].copy_from_slice(&1u16.to_le_bytes()); // 1 section
        v[p + 20..p + 22].copy_from_slice(&224u16.to_le_bytes()); // opt hdr
        let sect = p + 24 + 224;
        v[sect + 16..sect + 20].copy_from_slice(&512u32.to_le_bytes()); // size raw
        v[sect + 20..sect + 24].copy_from_slice(&1024u32.to_le_bytes()); // ptr raw
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("exec.pe").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, 1024 + 512);
    }
}
