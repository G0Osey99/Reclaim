//! Aho-Corasick signature matcher (docs/plan/03 §2.3).
//!
//! One automaton is built from every catalog signature's **anchor** (the
//! longest literal run in its header pattern). A hit yields the set of headers
//! sharing that anchor; the caller confirms each header's full masked pattern
//! and computes the implied file start. Single-byte anchors (only MPEG-TS's
//! `0x47`) are excluded — a one-byte key matches ~1/256 of all bytes and needs
//! a dedicated periodic-sync scanner instead (noted in phase-1.md).

use aho_corasick::{AhoCorasick, MatchKind};
use reclaim_sigs::{SigDef, SIGNATURES};
use std::collections::HashMap;

/// Points back into the catalog: signature index + header index.
#[derive(Copy, Clone, Debug)]
pub struct HeaderRef {
    /// Index into the signature slice.
    pub sig_idx: usize,
    /// Index into that signature's `headers`.
    pub header_idx: usize,
}

/// A confirmed-anchor candidate: the file is believed to start at `file_start`.
#[derive(Copy, Clone, Debug)]
pub struct Candidate {
    /// Index into the signature slice.
    pub sig_idx: usize,
    /// Index into that signature's `headers`.
    pub header_idx: usize,
    /// Absolute offset where the file is believed to begin.
    pub file_start: u64,
}

/// Minimum anchor length fed to the automaton.
pub const MIN_ANCHOR_LEN: usize = 2;

/// The compiled matcher.
pub struct Matcher {
    ac: AhoCorasick,
    refs: Vec<Vec<HeaderRef>>,
    sigs: &'static [SigDef],
}

impl std::fmt::Debug for Matcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Matcher")
            .field("patterns", &self.refs.len())
            .finish()
    }
}

impl Default for Matcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Matcher {
    /// Build the matcher over the full catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::for_signatures(SIGNATURES)
    }

    /// Build the matcher over an arbitrary signature slice (tests).
    // Constructing the automaton from the compile-time catalog is not on-disk
    // data; a failure here is a build bug, so a controlled panic is acceptable
    // (build guide Part 1.4 rule 3 concerns *parsed* data).
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    #[must_use]
    pub fn for_signatures(sigs: &'static [SigDef]) -> Self {
        let mut anchor_index: HashMap<&'static [u8], usize> = HashMap::new();
        let mut patterns: Vec<&'static [u8]> = Vec::new();
        let mut refs: Vec<Vec<HeaderRef>> = Vec::new();
        for (si, sig) in sigs.iter().enumerate() {
            for (hi, h) in sig.headers.iter().enumerate() {
                if h.anchor.len() < MIN_ANCHOR_LEN {
                    continue;
                }
                let id = *anchor_index.entry(h.anchor).or_insert_with(|| {
                    patterns.push(h.anchor);
                    refs.push(Vec::new());
                    patterns.len() - 1
                });
                if let Some(bucket) = refs.get_mut(id) {
                    bucket.push(HeaderRef {
                        sig_idx: si,
                        header_idx: hi,
                    });
                }
            }
        }
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::Standard)
            .build(&patterns)
            .expect("catalog anchors form a valid automaton");
        Matcher { ac, refs, sigs }
    }

    /// The signature slice this matcher was built over.
    #[must_use]
    pub fn signatures(&self) -> &'static [SigDef] {
        self.sigs
    }

    /// Call `f` for every candidate header in `buf`, whose first byte is at
    /// absolute offset `base`. Overlapping matches are reported so a byte that
    /// starts several anchors (e.g. `ftyp` shared by mp4/mov/heic) yields all.
    pub fn scan_buffer(&self, base: u64, buf: &[u8], mut f: impl FnMut(Candidate)) {
        for m in self.ac.find_overlapping_iter(buf) {
            let pid = m.pattern().as_usize();
            let anchor_abs = base.saturating_add(m.start() as u64);
            let Some(bucket) = self.refs.get(pid) else {
                continue;
            };
            for r in bucket {
                let Some(sig) = self.sigs.get(r.sig_idx) else {
                    continue;
                };
                let Some(h) = sig.headers.get(r.header_idx) else {
                    continue;
                };
                let Some(patt_start) = anchor_abs.checked_sub(h.anchor_pos as u64) else {
                    continue;
                };
                let Some(file_start) = patt_start.checked_sub(h.offset) else {
                    continue;
                };
                f(Candidate {
                    sig_idx: r.sig_idx,
                    header_idx: r.header_idx,
                    file_start,
                });
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn finds_jpeg_and_png_anchors() {
        let m = Matcher::new();
        // PNG magic then, later, a JPEG SOI.
        let mut buf = vec![0u8; 32];
        buf[0..8].copy_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        buf[16..20].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
        let mut hits: Vec<(String, u64)> = Vec::new();
        m.scan_buffer(1000, &buf, |c| {
            let sig = &SIGNATURES[c.sig_idx];
            hits.push((sig.id.to_string(), c.file_start));
        });
        assert!(hits
            .iter()
            .any(|(id, off)| id == "image.png" && *off == 1000));
        assert!(hits
            .iter()
            .any(|(id, off)| id == "image.jpeg" && *off == 1016));
    }

    #[test]
    fn computes_file_start_for_offset_anchor() {
        // ftyp anchor at pattern offset 4 → file starts 4 bytes earlier.
        let m = Matcher::new();
        let mut buf = vec![0u8; 32];
        buf[4..12].copy_from_slice(&[0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6F, 0x6D]); // ftyp isom
        let mut starts: Vec<u64> = Vec::new();
        m.scan_buffer(0, &buf, |c| {
            let sig = &SIGNATURES[c.sig_idx];
            if sig.id == "video.mp4" {
                starts.push(c.file_start);
            }
        });
        assert!(
            starts.contains(&0),
            "mp4 file_start should be 0, got {starts:?}"
        );
    }
}
