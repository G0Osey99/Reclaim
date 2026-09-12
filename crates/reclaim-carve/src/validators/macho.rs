//! Mach-O (docs/plan/05 §4): thin and fat. Walks load commands, tracking the
//! maximum `fileoff + filesize` of each segment for the size. `CA FE BA BE` is
//! disambiguated from a Java `.class` by the arch-count / version field.

use super::{Ctx, Verdict};
use crate::bytes::{be_u32, le_u32};
use crate::Validity;

const LC_SEGMENT: u32 = 0x1;
const LC_SEGMENT_64: u32 = 0x19;
const MAX_CMDS: u32 = 100_000;

/// Validate a Mach-O (or Java class, which shares the fat magic) candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 8);
    let magic = head.get(0..4).unwrap_or(&[]);
    let avail = ctx.available();
    match magic {
        [0xCA, 0xFE, 0xBA, 0xBE] => fat(ctx, avail),
        [0xFE, 0xED, 0xFA, 0xCE] => thin(ctx, avail, true, false),
        [0xFE, 0xED, 0xFA, 0xCF] => thin(ctx, avail, true, true),
        [0xCE, 0xFA, 0xED, 0xFE] => thin(ctx, avail, false, false),
        [0xCF, 0xFA, 0xED, 0xFE] => thin(ctx, avail, false, true),
        _ => Verdict::reject(),
    }
}

fn rd(be: bool, b: &[u8], o: usize) -> Option<u32> {
    if be {
        be_u32(b, o)
    } else {
        le_u32(b, o)
    }
}

fn thin(ctx: &Ctx, avail: u64, be: bool, is64: bool) -> Verdict {
    let hlen = if is64 { 32 } else { 28 };
    let head = ctx.read(0, hlen);
    let ncmds = rd(be, &head, 16).unwrap_or(0);
    let sizeofcmds = u64::from(rd(be, &head, 20).unwrap_or(0));
    if ncmds == 0 || ncmds > MAX_CMDS {
        return Verdict::reject();
    }
    let mut pos = hlen as u64;
    let mut max_extent = pos + sizeofcmds;
    for _ in 0..ncmds {
        let lc = ctx.read(pos, 8);
        let cmd = rd(be, &lc, 0).unwrap_or(0);
        let cmdsize = u64::from(rd(be, &lc, 4).unwrap_or(0));
        if cmdsize < 8 {
            break;
        }
        if cmd == LC_SEGMENT || cmd == LC_SEGMENT_64 {
            // fileoff/filesize at different offsets for 32 vs 64.
            let seg = ctx.read(pos, 72);
            let (fo, fs) = if cmd == LC_SEGMENT_64 {
                (
                    crate::bytes::be_u64(&seg, 40),
                    crate::bytes::be_u64(&seg, 48),
                )
            } else {
                (
                    rd(be, &seg, 32).map(u64::from),
                    rd(be, &seg, 36).map(u64::from),
                )
            };
            // 64-bit fields honor endianness too; recompute for LE.
            let (fo, fs) = if cmd == LC_SEGMENT_64 && !be {
                (
                    crate::bytes::le_u64(&seg, 40),
                    crate::bytes::le_u64(&seg, 48),
                )
            } else {
                (fo, fs)
            };
            if let (Some(o), Some(s)) = (fo, fs) {
                max_extent = max_extent.max(o.saturating_add(s));
            }
        }
        pos = pos.saturating_add(cmdsize);
    }
    finalize(max_extent, avail, "exec.macho", 85)
}

fn fat(ctx: &Ctx, avail: u64) -> Verdict {
    let head = ctx.read(0, 8);
    let nfat = be_u32(&head, 4).unwrap_or(0);
    // Java .class: bytes 4..8 are (minor<<16 | major), major ∈ 45..~70.
    if !(1..=32).contains(&nfat) {
        // Treat as Java class — size via constant pool is out of scope here.
        let len = avail.min(16 * 1024 * 1024);
        return Verdict::accept(len, Validity::Suspect, 40).with_format("exec.class");
    }
    let mut max_extent = 8u64 + u64::from(nfat) * 20;
    for i in 0..nfat as u64 {
        let a = ctx.read(8 + i * 20, 20);
        let off = u64::from(be_u32(&a, 8).unwrap_or(0));
        let size = u64::from(be_u32(&a, 12).unwrap_or(0));
        max_extent = max_extent.max(off.saturating_add(size));
    }
    finalize(max_extent, avail, "exec.macho", 88)
}

fn finalize(end: u64, avail: u64, id: &'static str, score: u8) -> Verdict {
    if end < 28 {
        return Verdict::reject();
    }
    if end <= avail {
        Verdict::accept(end, Validity::Full, score).with_format(id)
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
    fn fat_extent() {
        let mut v = vec![0xCA, 0xFE, 0xBA, 0xBE];
        v.extend_from_slice(&1u32.to_be_bytes()); // nfat=1
                                                  // arch: cputype, cpusubtype, offset=4096, size=100, align
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(&4096u32.to_be_bytes());
        v.extend_from_slice(&100u32.to_be_bytes());
        v.extend_from_slice(&12u32.to_be_bytes());
        v.resize(4096 + 100, 0);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("exec.macho").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, 4196);
    }

    #[test]
    fn java_class_disambiguated() {
        let mut v = vec![0xCA, 0xFE, 0xBA, 0xBE];
        v.extend_from_slice(&[0x00, 0x00, 0x00, 0x34]); // major 52 → not fat
        v.resize(2048, 0);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("exec.class").unwrap(),
        };
        let out = validate(&ctx);
        assert_eq!(out.format, Some("exec.class"));
    }
}
