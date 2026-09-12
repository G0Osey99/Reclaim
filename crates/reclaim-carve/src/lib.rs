//! Reclaim carving engine (docs/plan/03 §2.3, docs/plan/05).
//!
//! Pipeline: a sequential read over a read-only [`reclaim_block::BlockSource`]
//! feeds an Aho-Corasick [`matcher`] built from every catalog signature's
//! anchor; each candidate header is confirmed against its masked pattern and
//! handed to a per-format [`validators`] routine that walks the format's own
//! structure to determine the exact length and a [`Validity`]; the [`engine`]
//! infers the block size, applies the dedup/priority rules of doc 05 §2, and
//! emits [`CarvedFile`] records.
//!
//! Round 1 carves **contiguous** files only; fragmented media yields
//! [`Validity::Truncated`] results (fragment reassembly is round 2, doc 05 §5).
//!
//! This crate parses hostile on-disk bytes, so it denies panicking accessors
//! (build guide Part 1.4 rule 3): no `unwrap`/`expect`/indexing on data.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

pub mod bytes;
pub mod crc;
pub mod engine;
pub mod matcher;
pub mod metadata;
pub mod reader;
pub mod validators;

pub use engine::{CarveEngine, CarveOptions, CarveProgress, ScanOutcome};
pub use metadata::Metadata;
pub use reader::{MemReader, Reader, SourceReader};

use serde::{Deserialize, Serialize};

/// How complete/trustworthy a carved result is (docs/plan/05 §1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Validity {
    /// The validator reached a clean logical end of file.
    Full,
    /// The structure was valid but ran out of data (media truncated / the file
    /// is fragmented — round 1 does not reassemble).
    Truncated,
    /// Accepted on weak evidence, or touched a bad/unread sector.
    Suspect,
}

impl Validity {
    /// Lowercase label for output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Validity::Full => "full",
            Validity::Truncated => "truncated",
            Validity::Suspect => "suspect",
        }
    }
}

/// A file recovered by the carver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarvedFile {
    /// Absolute byte offset of the file start in the source.
    pub offset: u64,
    /// Length in bytes.
    pub len: u64,
    /// Refined signature id (`family.format`).
    pub format: String,
    /// Family (`image`, `video`, …).
    pub family: String,
    /// Preferred extension for naming.
    pub ext: String,
    /// Completeness.
    pub validity: Validity,
    /// Confidence 0–100.
    pub score: u8,
    /// Embedded metadata for naming (docs/plan/05 §6).
    pub meta: Metadata,
    /// True if the header sat on the inferred block boundary.
    pub block_aligned: bool,
}

impl CarvedFile {
    /// One-past-the-end offset.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(self.len)
    }
}
