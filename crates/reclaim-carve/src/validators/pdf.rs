//! PDF (docs/plan/05 §4.6): header → the *last* `%%EOF` (incremental updates
//! append several) within the size cap; `startxref` sanity raises the score.

use super::{Ctx, Verdict};
use crate::Validity;

const MAX: u64 = 512 * 1024 * 1024;
const WIN: usize = 1 << 20;
const EOF: &[u8] = b"%%EOF";

/// Validate a PDF candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    if ctx.read(0, 5) != *b"%PDF-" {
        return Verdict::reject();
    }
    let cap = ctx.available().min(MAX);
    let mut pos: u64 = 0;
    let mut last_eof_end: Option<u64> = None;
    let mut saw_startxref = false;

    while pos < cap {
        let want = (cap - pos).min(WIN as u64) as usize;
        let buf = ctx.read(pos, want);
        if buf.is_empty() {
            break;
        }
        for m in memchr::memmem::find_iter(&buf, EOF) {
            let mut end = pos + m as u64 + EOF.len() as u64;
            // Consume a trailing CR/LF.
            let tail = ctx.read(end, 2);
            if tail.first() == Some(&b'\r') && tail.get(1) == Some(&b'\n') {
                end += 2;
            } else if matches!(tail.first(), Some(b'\n') | Some(b'\r')) {
                end += 1;
            }
            last_eof_end = Some(end);
        }
        if memchr::memmem::find(&buf, b"startxref").is_some() {
            saw_startxref = true;
        }
        if want < WIN {
            break;
        }
        pos += (want - EOF.len()) as u64; // overlap so a straddling %%EOF is found
    }

    match last_eof_end {
        Some(end) if end >= 64 => {
            let score = if saw_startxref { 90 } else { 72 };
            Verdict::accept(end.min(cap), Validity::Full, score)
        }
        _ => {
            if cap < 64 {
                Verdict::reject()
            } else {
                Verdict::accept(cap, Validity::Truncated, 45)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    #[test]
    fn stops_at_last_eof() {
        let mut v = Vec::from(&b"%PDF-1.7\n"[..]);
        v.extend_from_slice(&[b'x'; 200]);
        v.extend_from_slice(b"\nstartxref\n9\n%%EOF\n");
        // incremental update
        v.extend_from_slice(&[b'y'; 50]);
        v.extend_from_slice(b"\nstartxref\n250\n%%EOF\n");
        let end = v.len() as u64;
        v.extend_from_slice(&[0u8; 512]);
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("doc.pdf").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.len, end);
        assert_eq!(out.validity, Validity::Full);
    }
}
