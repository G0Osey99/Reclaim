//! Text classifier (docs/plan/05 §4.9): headerless plain text.
//!
//! Plain text has no magic, so the engine's statistical pass offers candidates
//! at block boundaries where [`looks_like_text`] flips from false to true. This
//! validator then walks the contiguous run of text bytes (ASCII printable/space
//! plus valid non-control UTF-8) to the first binary byte — which, for a
//! deleted file whose cluster slack is zeroed, is exactly the file's end.

use super::{Ctx, Verdict};
use crate::Validity;

const MAX: u64 = 256 * 1024 * 1024;
const WIN: usize = 64 * 1024;
/// Shortest run accepted as a text file.
const MIN_RUN: u64 = 32;

/// Validate a text candidate: measure the contiguous text run from `file_start`.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let cap = ctx.available().min(MAX);
    if cap < MIN_RUN {
        return Verdict::reject();
    }
    // The start must itself look like text. Sample up to the first binary byte
    // so a short text file whose cluster slack is zeroed still classifies as
    // text (the slack would otherwise drown the printable ratio).
    let head = ctx.read(0, 512.min(cap as usize));
    let sample_end = head.iter().position(|b| *b < 0x09).unwrap_or(head.len());
    let sample = head.get(..sample_end.max(1)).unwrap_or(&head);
    if !looks_like_text(sample) {
        return Verdict::reject();
    }

    let mut pos: u64 = 0;
    let mut printable: u64 = 0;
    let mut hit_binary = false;
    'outer: while pos < cap {
        let want = (cap - pos).min(WIN as u64) as usize;
        let buf = ctx.read(pos, want);
        if buf.is_empty() {
            break;
        }
        let mut i = 0usize;
        while i < buf.len() {
            let b = *buf.get(i).unwrap_or(&0);
            if b < 0x80 {
                if is_ascii_text(b) {
                    printable += 1;
                    i += 1;
                    continue;
                }
                // Binary byte → end of text run.
                pos += i as u64;
                hit_binary = true;
                break 'outer;
            }
            // Possible UTF-8 multibyte lead.
            match utf8_len(b) {
                Some(n) if i + n <= buf.len() => {
                    if valid_utf8_seq(buf.get(i..i + n).unwrap_or(&[])) {
                        printable += 1;
                        i += n;
                    } else {
                        pos += i as u64;
                        hit_binary = true;
                        break 'outer;
                    }
                }
                Some(n) => {
                    // Sequence straddles the window; re-read from here.
                    if want == WIN && i > 0 {
                        pos += i as u64;
                        continue 'outer;
                    }
                    let _ = n;
                    pos += i as u64;
                    hit_binary = true;
                    break 'outer;
                }
                None => {
                    pos += i as u64;
                    hit_binary = true;
                    break 'outer;
                }
            }
        }
        if want < WIN {
            // reached cap
            pos = cap;
            break;
        }
        pos += buf.len() as u64;
    }

    if pos < MIN_RUN {
        return Verdict::reject();
    }
    let ratio = printable as f64 / pos as f64;
    if ratio < 0.60 {
        return Verdict::reject();
    }
    let (validity, mut score) = if hit_binary {
        (Validity::Full, 68) // ended at a clean binary boundary (e.g. slack)
    } else {
        (Validity::Truncated, 50) // ran to the cap / end of source
    };
    // A little index-of-coincidence-style bonus for language-like text.
    if ratio > 0.98 {
        score += 6;
    }
    let id = refine(&head);
    Verdict::accept(pos, validity, score).with_format(id)
}

/// Narrow the id by the leading bytes (XML/plist/csv-ish/plain).
fn refine(head: &[u8]) -> &'static str {
    if head.starts_with(b"<?xml") {
        if window_contains(head, b"<plist") {
            "sys.plist_xml"
        } else {
            "doc.xml"
        }
    } else if head.starts_with(b"<html") || head.starts_with(b"<!DOCTYPE") {
        "doc.html"
    } else if head.starts_with(b"{") || head.starts_with(b"[") {
        "doc.json"
    } else {
        "doc.txt"
    }
}

fn window_contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Block-level classifier used by the engine's statistical pass.
#[must_use]
pub fn looks_like_text(buf: &[u8]) -> bool {
    if buf.len() < 16 {
        return false;
    }
    let mut printable = 0usize;
    let mut letters = 0usize;
    let mut first = None;
    let mut uniform = true;
    for &b in buf {
        match first {
            None => first = Some(b),
            Some(f) if f != b => uniform = false,
            _ => {}
        }
        if is_ascii_text(b) || b >= 0x80 {
            printable += 1;
        }
        if b.is_ascii_alphabetic() || b == b' ' {
            letters += 1;
        }
    }
    if uniform {
        return false;
    }
    let n = buf.len();
    (printable as f64 / n as f64) >= 0.92 && (letters as f64 / n as f64) >= 0.20
}

fn is_ascii_text(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E)
}

fn utf8_len(lead: u8) -> Option<usize> {
    match lead {
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}

fn valid_utf8_seq(s: &[u8]) -> bool {
    std::str::from_utf8(s)
        .map(|d| !d.chars().any(|c| c.is_control()))
        .unwrap_or(false)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    fn ctx_for<'a>(r: &'a MemReader<'a>) -> Ctx<'a> {
        Ctx {
            reader: r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("doc.txt").unwrap(),
        }
    }

    #[test]
    fn recovers_exact_text_run_before_zero_slack() {
        let text =
            b"reclaim recovery sector cluster inode extent carve signature\nvolume checkpoint\n";
        let mut data = Vec::from(&text[..]);
        data.extend_from_slice(&[0u8; 4096]); // cluster slack
        let r = MemReader::new(&data);
        let v = validate(&ctx_for(&r));
        assert!(v.accept);
        assert_eq!(v.validity, Validity::Full);
        assert_eq!(v.len, text.len() as u64);
    }

    #[test]
    fn rejects_binary() {
        let data: Vec<u8> = (0..512u32).map(|i| (i * 37 % 256) as u8).collect();
        let r = MemReader::new(&data);
        assert!(!validate(&ctx_for(&r)).accept);
    }

    #[test]
    fn classifier_flags_text_only() {
        assert!(looks_like_text(
            b"the quick brown fox jumps over the lazy dog again"
        ));
        assert!(!looks_like_text(&[0u8; 64]));
        assert!(!looks_like_text(&[0xAB; 64]));
    }
}
