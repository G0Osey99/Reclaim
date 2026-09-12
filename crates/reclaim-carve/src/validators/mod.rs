//! Per-format validators (docs/plan/05 §4).
//!
//! A validator inspects a candidate file starting at `ctx.file_start` via the
//! random-access [`Reader`], walks the format's own structure, and returns a
//! [`Verdict`] with the exact recovered length, a [`Validity`], a confidence
//! score, and — for container magics shared by several formats (RIFF, ZIP,
//! ISOBMFF, TIFF) — the refined signature id in `format`.
//!
//! Validators never panic on input (the crate denies `unwrap`/`expect`/indexing).

use crate::metadata::Metadata;
use crate::reader::Reader;
use crate::Validity;
use reclaim_sigs::SigDef;

pub mod bmp;
pub mod bplist;
pub mod bzip2;
pub mod dmg;
pub mod elf;
pub mod flac;
pub mod gif;
pub mod gzip;
pub mod ico;
pub mod iff;
pub mod iso;
pub mod isobmff;
pub mod jpeg;
pub mod macho;
pub mod mkv;
pub mod mp3;
pub mod mpegts;
pub mod ogg;
pub mod ole2;
pub mod pdf;
pub mod pe;
pub mod png;
pub mod rar;
pub mod riff;
pub mod sevenz;
pub mod sqlite;
pub mod tar;
pub mod text;
pub mod tiff;
pub mod wasm;
pub mod xz;
pub mod zip;
pub mod zstd;

/// Input to a validator.
#[derive(Debug)]
pub struct Ctx<'a> {
    /// Random-access reader over the source.
    pub reader: &'a dyn Reader,
    /// Absolute offset where the candidate file begins.
    pub file_start: u64,
    /// Hard cap on the recovered length (min of source tail and max_file_size).
    pub max_len: u64,
    /// The catalog signature that produced this candidate.
    pub sig: &'static SigDef,
}

impl Ctx<'_> {
    /// Read `len` bytes at `file_start + rel`.
    #[must_use]
    pub fn read(&self, rel: u64, len: usize) -> Vec<u8> {
        self.reader.read(self.file_start.saturating_add(rel), len)
    }

    /// Read `len` bytes at `file_start + rel`, also reporting whether the range
    /// touched a bad/unread sector.
    #[must_use]
    pub fn read_flags(&self, rel: u64, len: usize) -> (Vec<u8>, bool) {
        self.reader
            .read_flags(self.file_start.saturating_add(rel), len)
    }

    /// Bytes remaining from `file_start` to end of source, capped at `max_len`.
    #[must_use]
    pub fn available(&self) -> u64 {
        self.reader
            .len()
            .saturating_sub(self.file_start)
            .min(self.max_len)
    }
}

/// A validator's decision.
#[derive(Clone, Debug)]
pub struct Verdict {
    /// Whether this is a real file of the claimed family.
    pub accept: bool,
    /// Recovered length in bytes.
    pub len: u64,
    /// Completeness.
    pub validity: Validity,
    /// Confidence 0–100.
    pub score: u8,
    /// Refined signature id, if the validator narrowed a shared container magic.
    pub format: Option<&'static str>,
    /// For footer-anchored formats (DMG `koly`): the true file start, computed
    /// backwards from a trailer. When `None`, the file starts at `ctx.file_start`.
    pub start_override: Option<u64>,
    /// Extracted naming metadata.
    pub meta: Metadata,
}

impl Verdict {
    /// A rejection.
    #[must_use]
    pub fn reject() -> Self {
        Verdict {
            accept: false,
            len: 0,
            validity: Validity::Suspect,
            score: 0,
            format: None,
            start_override: None,
            meta: Metadata::default(),
        }
    }

    /// An acceptance.
    #[must_use]
    pub fn accept(len: u64, validity: Validity, score: u8) -> Self {
        Verdict {
            accept: true,
            len,
            validity,
            score,
            format: None,
            start_override: None,
            meta: Metadata::default(),
        }
    }

    /// Set the true file start for a footer-anchored format (builder).
    #[must_use]
    pub fn with_start(mut self, start: u64) -> Self {
        self.start_override = Some(start);
        self
    }

    /// Set the refined format id (builder).
    #[must_use]
    pub fn with_format(mut self, id: &'static str) -> Self {
        self.format = Some(id);
        self
    }

    /// Set the metadata (builder).
    #[must_use]
    pub fn with_meta(mut self, meta: Metadata) -> Self {
        self.meta = meta;
        self
    }
}

/// Dispatch to the named validator (from `SigDef::validator`).
#[must_use]
pub fn dispatch(name: &str, ctx: &Ctx) -> Verdict {
    match name {
        "jpeg" => jpeg::validate(ctx),
        "png" => png::validate(ctx),
        "gif" => gif::validate(ctx),
        "ico" => ico::validate(ctx),
        "bmp" => bmp::validate(ctx),
        "tiff" => tiff::validate(ctx),
        "isobmff" => isobmff::validate(ctx),
        "riff" => riff::validate(ctx),
        "iff" => iff::validate(ctx),
        "zip" => zip::validate(ctx),
        "ole2" => ole2::validate(ctx),
        "pdf" => pdf::validate(ctx),
        "sqlite" => sqlite::validate(ctx),
        "text" => text::validate(ctx),
        "gzip" => gzip::validate(ctx),
        "bzip2" => bzip2::validate(ctx),
        "xz" => xz::validate(ctx),
        "zstd" => zstd::validate(ctx),
        "sevenz" => sevenz::validate(ctx),
        "rar" => rar::validate(ctx),
        "tar" => tar::validate(ctx),
        "macho" => macho::validate(ctx),
        "elf" => elf::validate(ctx),
        "pe" => pe::validate(ctx),
        "dmg" => dmg::validate(ctx),
        "bplist" => bplist::validate(ctx),
        "mp3" => mp3::validate(ctx),
        "flac" => flac::validate(ctx),
        "ogg" => ogg::validate(ctx),
        "mkv" => mkv::validate(ctx),
        "mpegts" => mpegts::validate(ctx),
        "iso" => iso::validate(ctx),
        "wasm" => wasm::validate(ctx),
        // psd/font/dds/xar/rtf fall back to strategy-based extraction.
        _ => Verdict::reject(),
    }
}

/// True if the named validator is registered.
#[must_use]
pub fn has_validator(name: &str) -> bool {
    matches!(
        name,
        "jpeg"
            | "png"
            | "gif"
            | "ico"
            | "bmp"
            | "tiff"
            | "isobmff"
            | "riff"
            | "iff"
            | "zip"
            | "ole2"
            | "pdf"
            | "sqlite"
            | "text"
            | "gzip"
            | "bzip2"
            | "xz"
            | "zstd"
            | "sevenz"
            | "rar"
            | "tar"
            | "macho"
            | "elf"
            | "pe"
            | "dmg"
            | "bplist"
            | "mp3"
            | "flac"
            | "ogg"
            | "mkv"
            | "mpegts"
            | "iso"
            | "wasm"
    )
}
