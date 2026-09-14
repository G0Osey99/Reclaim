//! The `.reclaim-map` sidecar (docs/plan/02 FR-IMG-1): geometry, per-chunk and
//! whole-image hashes, bad/unread LBA ranges, optional compression frame index,
//! and enough source identity (first/last-MiB hashes) to re-attach on resume
//! (docs/plan/07 §4).

use crate::bad_block::LbaRange;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Whole/per-chunk hash algorithm.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgo {
    /// BLAKE3 (default; fast).
    Blake3,
    /// SHA-256 (forensic interchange).
    Sha256,
}

impl HashAlgo {
    /// Parse a `--hash` argument.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "blake3" | "b3" => Some(HashAlgo::Blake3),
            "sha256" | "sha-256" => Some(HashAlgo::Sha256),
            _ => None,
        }
    }
    /// Label for output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            HashAlgo::Blake3 => "blake3",
            HashAlgo::Sha256 => "sha256",
        }
    }
}

/// Destination compression.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    /// Raw (possibly sparse) output.
    None,
    /// zstd-framed output with a random-access frame index.
    Zstd,
}

/// One zstd frame's placement.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameEntry {
    /// Logical image offset the frame decompresses to.
    pub image_offset: u64,
    /// Byte offset of the frame in the output file.
    pub file_offset: u64,
    /// Uncompressed length.
    pub uncompressed: u32,
    /// Compressed length.
    pub compressed: u32,
}

/// The imaging sidecar map.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageMap {
    /// Format version.
    pub version: u32,
    /// Source identity string.
    pub source_id: String,
    /// Logical source size in bytes.
    pub size: u64,
    /// Logical sector size.
    pub sector_size: u32,
    /// Hashing / imaging chunk size in bytes.
    pub chunk_size: u32,
    /// Hash algorithm.
    pub hash_algo: HashAlgo,
    /// Output compression.
    pub compression: Compression,
    /// zstd frame index (empty for raw output).
    #[serde(default)]
    pub frames: Vec<FrameEntry>,
    /// Per-chunk hash (hex); `None` = not yet imaged (resume marker).
    #[serde(default)]
    pub chunk_hashes: Vec<Option<String>>,
    /// Whole-image hash (hex) once complete.
    #[serde(default)]
    pub whole_hash: Option<String>,
    /// Bad (unreadable) LBA runs.
    #[serde(default)]
    pub bad: Vec<LbaRange>,
    /// Never-read LBA runs (interrupted imaging).
    #[serde(default)]
    pub unread: Vec<LbaRange>,
    /// True once every chunk is imaged.
    pub complete: bool,
    /// Hash of the first MiB (source re-identification).
    #[serde(default)]
    pub first_mib_hash: Option<String>,
    /// Hash of the last MiB.
    #[serde(default)]
    pub last_mib_hash: Option<String>,
}

impl ImageMap {
    /// Create a fresh map for a source.
    #[must_use]
    pub fn new(
        source_id: String,
        size: u64,
        sector_size: u32,
        chunk_size: u32,
        hash_algo: HashAlgo,
        compression: Compression,
    ) -> Self {
        let num = size.div_ceil(u64::from(chunk_size.max(1))) as usize;
        ImageMap {
            version: 1,
            source_id,
            size,
            sector_size,
            chunk_size,
            hash_algo,
            compression,
            frames: Vec::new(),
            chunk_hashes: vec![None; num],
            whole_hash: None,
            bad: Vec::new(),
            unread: Vec::new(),
            complete: false,
            first_mib_hash: None,
            last_mib_hash: None,
        }
    }

    /// Number of chunks.
    #[must_use]
    pub fn num_chunks(&self) -> usize {
        self.chunk_hashes.len()
    }

    /// True if chunk `i` has been imaged.
    #[must_use]
    pub fn is_chunk_done(&self, i: usize) -> bool {
        self.chunk_hashes
            .get(i)
            .map(Option::is_some)
            .unwrap_or(false)
    }

    /// Total bad sectors recorded.
    #[must_use]
    pub fn bad_sectors(&self) -> u64 {
        self.bad.iter().map(|r| r.count).sum()
    }

    /// Load a map from JSON on disk.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Save the map as pretty JSON. Written to `<path>.tmp` and renamed into
    /// place so an interrupted save never leaves a truncated map behind
    /// (resume would otherwise refuse to load it).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut m = ImageMap::new(
            "device:disk9:1000".into(),
            1000,
            512,
            256,
            HashAlgo::Blake3,
            Compression::None,
        );
        assert_eq!(m.num_chunks(), 4);
        m.chunk_hashes[0] = Some("abcd".into());
        assert!(m.is_chunk_done(0));
        assert!(!m.is_chunk_done(1));
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.map");
        m.save(&p).unwrap();
        let back = ImageMap::load(&p).unwrap();
        assert_eq!(back.size, 1000);
        assert_eq!(back.chunk_hashes[0].as_deref(), Some("abcd"));
    }

    #[test]
    fn hash_algo_parse() {
        assert_eq!(HashAlgo::parse("blake3"), Some(HashAlgo::Blake3));
        assert_eq!(HashAlgo::parse("SHA-256"), Some(HashAlgo::Sha256));
        assert_eq!(HashAlgo::parse("md5"), None);
    }
}
