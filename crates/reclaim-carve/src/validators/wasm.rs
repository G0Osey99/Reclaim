//! WebAssembly (docs/plan/05 §3.8): `\0asm` + version, then a section walk
//! (`id` byte + LEB128 size + payload) to the end (exact size).

use super::{Ctx, Verdict};
use crate::Validity;

const MAX: u64 = 512 * 1024 * 1024;

/// Validate a WASM candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 8);
    if head.get(0..4) != Some(&[0x00, 0x61, 0x73, 0x6D]) {
        return Verdict::reject();
    }
    if head.get(4..8) != Some(&[0x01, 0x00, 0x00, 0x00]) {
        return Verdict::reject();
    }
    let cap = ctx.available().min(MAX);
    let mut pos: u64 = 8;
    for _ in 0..100_000 {
        if pos >= cap {
            break;
        }
        let intro = ctx.read(pos, 6);
        let id = match intro.first() {
            Some(b) if *b <= 13 => *b,
            _ => break, // not a valid section id → end
        };
        let _ = id;
        let (size, n) = match read_uleb(intro.get(1..).unwrap_or(&[])) {
            Some(x) => x,
            None => break,
        };
        let next = pos + 1 + n as u64 + size;
        if next > cap {
            return Verdict::accept(cap, Validity::Truncated, 45).with_format("exec.wasm");
        }
        pos = next;
    }
    if pos < 8 {
        return Verdict::reject();
    }
    Verdict::accept(pos, Validity::Full, 85).with_format("exec.wasm")
}

fn read_uleb(b: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    for i in 0..b.len().min(10) {
        let byte = *b.get(i)?;
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn walks_sections() {
        let mut v = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        // one section: id=1, size=3, payload
        v.push(1);
        v.push(3);
        v.extend_from_slice(&[0, 0, 0]);
        let end = v.len() as u64;
        v.extend_from_slice(&[0xFFu8; 16]); // invalid section id → stop
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("exec.wasm").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, end);
    }
}
