//! ELF (docs/plan/05 §4): size from the section-header table extent
//! (`e_shoff + e_shnum·e_shentsize`) and program-header file extents.

use super::{Ctx, Verdict};
use crate::bytes::{be_u16, be_u32, be_u64, le_u16, le_u32, le_u64};
use crate::Validity;

/// Validate an ELF candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 64);
    if head.get(0..4) != Some(&[0x7F, 0x45, 0x4C, 0x46]) {
        return Verdict::reject();
    }
    let is64 = head.get(4).copied() == Some(2);
    let be = head.get(5).copied() == Some(2);
    let u16 = |o: usize| {
        if be {
            be_u16(&head, o)
        } else {
            le_u16(&head, o)
        }
    };
    let u32 = |o: usize| {
        if be {
            be_u32(&head, o)
        } else {
            le_u32(&head, o)
        }
    };
    let u64f = |o: usize| {
        if be {
            be_u64(&head, o)
        } else {
            le_u64(&head, o)
        }
    };

    let (e_phoff, e_shoff, e_phentsize, e_phnum, e_shentsize, e_shnum) = if is64 {
        (
            u64f(0x20).unwrap_or(0),
            u64f(0x28).unwrap_or(0),
            u16(0x36).unwrap_or(0) as u64,
            u16(0x38).unwrap_or(0) as u64,
            u16(0x3A).unwrap_or(0) as u64,
            u16(0x3C).unwrap_or(0) as u64,
        )
    } else {
        (
            u64::from(u32(0x1C).unwrap_or(0)),
            u64::from(u32(0x20).unwrap_or(0)),
            u16(0x2A).unwrap_or(0) as u64,
            u16(0x2C).unwrap_or(0) as u64,
            u16(0x2E).unwrap_or(0) as u64,
            u16(0x30).unwrap_or(0) as u64,
        )
    };

    let mut end = if is64 { 64 } else { 52 };
    end = end.max(e_shoff.saturating_add(e_shnum.saturating_mul(e_shentsize)));

    // Program headers give segment file extents.
    if e_phnum > 0 && e_phnum < 65536 && e_phentsize >= 8 {
        for i in 0..e_phnum {
            let off = e_phoff.saturating_add(i.saturating_mul(e_phentsize));
            let ph = ctx.read(off, e_phentsize.min(64) as usize);
            let (p_offset, p_filesz) = if is64 {
                (
                    if be { be_u64(&ph, 8) } else { le_u64(&ph, 8) },
                    if be { be_u64(&ph, 32) } else { le_u64(&ph, 32) },
                )
            } else {
                (
                    if be {
                        be_u32(&ph, 4).map(u64::from)
                    } else {
                        le_u32(&ph, 4).map(u64::from)
                    },
                    if be {
                        be_u32(&ph, 16).map(u64::from)
                    } else {
                        le_u32(&ph, 16).map(u64::from)
                    },
                )
            };
            if let (Some(o), Some(s)) = (p_offset, p_filesz) {
                end = end.max(o.saturating_add(s));
            }
        }
    }

    let avail = ctx.available();
    if end < 52 {
        return Verdict::reject();
    }
    if end <= avail {
        Verdict::accept(end, Validity::Full, 85).with_format("exec.elf")
    } else {
        Verdict::accept(avail, Validity::Truncated, 50).with_format("exec.elf")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn section_header_extent_64() {
        let mut v = vec![0u8; 4096];
        v[0..4].copy_from_slice(&[0x7F, 0x45, 0x4C, 0x46]);
        v[4] = 2; // 64-bit
        v[5] = 1; // LE
        v[0x28..0x30].copy_from_slice(&2048u64.to_le_bytes()); // e_shoff
        v[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
        v[0x3C..0x3E].copy_from_slice(&4u16.to_le_bytes()); // e_shnum
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("exec.elf").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, 2048 + 64 * 4);
    }
}
