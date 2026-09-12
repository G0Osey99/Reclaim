//! Reclaim file-signature catalog (docs/plan/05 §1).
//!
//! Signatures are **data**: `catalog/*.toml` files describe each format's
//! magic bytes, footer, size bounds, extraction strategy and the name of the
//! Rust validator that refines/sizes it. `build.rs` parses and validates every
//! TOML at build time and codegens a static [`SigDef`] table into `OUT_DIR`,
//! included below. Adding a header-only format is therefore a TOML change with
//! no code (build guide Part 3.4).
//!
//! The matcher (in `reclaim-carve`) builds an Aho-Corasick automaton over each
//! signature's **anchor** — the longest literal byte run inside its header
//! pattern — then confirms the full (wildcard-masked) pattern at the candidate
//! offset. This module only defines the data model and the compiled table.
#![forbid(unsafe_code)]

/// Extraction strategy for a signature (docs/plan/05 §1).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Constant size (`min_size`).
    Fixed,
    /// Size is parsed from a header field by the validator.
    HeaderSize,
    /// Scan forward for the footer within `search_max`; take the smallest valid.
    Footer,
    /// The validator walks the format's own structure to the logical end.
    Structural,
    /// Headerless text: byte-class / index-of-coincidence classifier.
    Statistical,
}

/// One header pattern for a signature.
///
/// `bytes` is the full pattern with wildcard positions zeroed; `mask` is
/// `0xFF` at literal positions and `0x00` at wildcards. The pattern sits at
/// byte `offset` relative to the start of the file. `anchor` is the maximal
/// literal run within `bytes` (used as the Aho-Corasick search key) and
/// `anchor_pos` is its index within `bytes`.
#[derive(Copy, Clone, Debug)]
pub struct HeaderPat {
    /// Full pattern bytes (wildcards zeroed).
    pub bytes: &'static [u8],
    /// Match mask: `0xFF` literal, `0x00` wildcard.
    pub mask: &'static [u8],
    /// Pattern position relative to file start.
    pub offset: u64,
    /// Literal run used as the Aho-Corasick anchor.
    pub anchor: &'static [u8],
    /// Index of `anchor` within `bytes`.
    pub anchor_pos: usize,
}

impl HeaderPat {
    /// True if `window` (which must begin at the pattern position, i.e. file
    /// offset + [`HeaderPat::offset`]) matches this pattern under the mask.
    #[must_use]
    pub fn confirm(&self, window: &[u8]) -> bool {
        if window.len() < self.bytes.len() {
            return false;
        }
        self.bytes
            .iter()
            .zip(self.mask.iter())
            .zip(window.iter())
            .all(|((b, m), w)| (w & m) == (b & m))
    }
}

/// A footer pattern for `footer`-strategy signatures.
#[derive(Copy, Clone, Debug)]
pub struct FooterPat {
    /// Footer bytes (literal).
    pub bytes: &'static [u8],
    /// Maximum distance from the header to search for the footer.
    pub search_max: u64,
}

/// A compiled signature definition.
#[derive(Copy, Clone, Debug)]
pub struct SigDef {
    /// Stable dotted id, `family.format`.
    pub id: &'static str,
    /// Family (`image`, `video`, `raw`, `doc`, …).
    pub family: &'static str,
    /// Associated file extensions (first is preferred for naming).
    pub extensions: &'static [&'static str],
    /// MIME type (may be empty).
    pub mime: &'static str,
    /// Catalog tier (1 = validated at launch, 2/3 = header/footer only).
    pub tier: u8,
    /// Header patterns (any match is a candidate).
    pub headers: &'static [HeaderPat],
    /// Optional footer for `footer` strategy.
    pub footer: Option<FooterPat>,
    /// Minimum plausible size in bytes.
    pub min_size: u64,
    /// Maximum plausible size in bytes.
    pub max_size: u64,
    /// Extraction strategy.
    pub strategy: Strategy,
    /// Name of the Rust validator (registered in `reclaim-carve`), if any.
    pub validator: Option<&'static str>,
    /// Metadata extractors for naming (docs/plan/05 §6).
    pub metadata: &'static [&'static str],
    /// Fast path expects the header only at a block/cluster boundary.
    pub block_aligned: bool,
    /// Detected by a trailer magic near EOF; start is computed backwards
    /// (DMG `koly`, TGA). The `anchor` for these is the trailer bytes.
    pub footer_anchored: bool,
    /// Participates in fragment reassembly (round 2).
    pub reassembly: bool,
}

impl SigDef {
    /// Preferred file extension for naming (first listed, or `"bin"`).
    #[must_use]
    pub fn primary_ext(&self) -> &'static str {
        self.extensions.first().copied().unwrap_or("bin")
    }
}

// Codegen: `pub static SIGNATURES: &[SigDef] = &[ … ];` plus `MAX_ANCHOR_LEN`
// and `MAX_HEADER_SPAN` constants (build.rs, from catalog/*.toml).
include!(concat!(env!("OUT_DIR"), "/catalog.rs"));

/// All compiled signatures.
#[must_use]
pub fn all() -> &'static [SigDef] {
    SIGNATURES
}

/// Look up a signature by its dotted id.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static SigDef> {
    SIGNATURES.iter().find(|s| s.id == id)
}

/// All signatures in a family.
pub fn by_family(family: &str) -> Vec<&'static SigDef> {
    SIGNATURES.iter().filter(|s| s.family == family).collect()
}

/// The set of distinct families present in the catalog, in first-seen order.
#[must_use]
pub fn families() -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for s in SIGNATURES {
        if !seen.contains(&s.family) {
            seen.push(s.family);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_populated() {
        assert!(
            SIGNATURES.len() >= 120,
            "expected >= 120 seeded signatures, got {}",
            SIGNATURES.len()
        );
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = SIGNATURES.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate signature id in catalog");
    }

    #[test]
    fn every_header_has_a_nonempty_anchor() {
        for s in SIGNATURES {
            for h in s.headers {
                assert!(!h.anchor.is_empty(), "{} has an empty anchor", s.id);
                assert_eq!(h.bytes.len(), h.mask.len(), "{} bytes/mask length", s.id);
                assert!(h.anchor_pos + h.anchor.len() <= h.bytes.len(), "{}", s.id);
            }
        }
    }

    #[test]
    fn jpeg_png_mp4_present_and_validated() {
        for id in ["image.jpeg", "image.png", "video.mp4", "doc.txt"] {
            let s = by_id(id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(s.tier == 1, "{id} should be Tier 1");
        }
    }
}
