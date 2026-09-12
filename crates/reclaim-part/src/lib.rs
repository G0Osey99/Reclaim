//! Reclaim partition-scheme parser (docs/plan/04 §1).
//!
//! [`scan`] reads a read-only [`reclaim_block::BlockSource`] and returns a
//! [`PartitionMap`]: GPT (primary + backup header, CRC-32 validated, Apple type
//! GUIDs recognised), MBR with EBR extended-partition chains, hybrid-MBR
//! detection, Apple Partition Map (detect-only), and APFS-container / Core
//! Storage detection (detect-only — consumed by the Phase 3/4 engines). The
//! planner turns each [`Partition`] into an `OffsetView` for the metadata
//! engines, and `reclaim info` prints the map.
//!
//! Parses hostile on-disk bytes; denies panicking accessors (build guide
//! Part 1.4 rule 3).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

pub mod crc;
pub mod guid;

use reclaim_block::BlockSource;
use serde::Serialize;
use std::sync::Arc;

/// Partition-table scheme.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    /// GUID Partition Table.
    Gpt,
    /// Classic MBR (with any EBR extended chains).
    Mbr,
    /// An MBR alongside a GPT that maps real partitions (not merely protective).
    HybridMbr,
    /// Apple Partition Map (detect-only).
    Apm,
    /// No recognised partition table — the whole source is treated as one
    /// volume (e.g. a floppy-style / superfloppy filesystem image).
    None,
}

impl Scheme {
    /// Lowercase label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Scheme::Gpt => "gpt",
            Scheme::Mbr => "mbr",
            Scheme::HybridMbr => "hybrid-mbr",
            Scheme::Apm => "apm",
            Scheme::None => "none",
        }
    }
}

/// One partition / volume window within the source.
#[derive(Clone, Debug, Serialize)]
pub struct Partition {
    /// 1-based index within its scheme.
    pub index: u32,
    /// Byte offset of the partition start in the source.
    pub start: u64,
    /// Length in bytes.
    pub len: u64,
    /// Scheme that produced this entry (distinguishes hybrid-MBR members).
    pub scheme: Scheme,
    /// MBR type byte, if from an MBR/EBR.
    pub type_byte: Option<u8>,
    /// GPT type GUID string, if from a GPT.
    pub type_guid: Option<String>,
    /// Human type label.
    pub type_label: String,
    /// GPT partition name (UTF-16LE), if any.
    pub name: Option<String>,
    /// A coarse filesystem hint for the planner (`"exfat/ntfs"`, `"fat"`,
    /// `"apfs"`, …); the engines still probe to be sure.
    pub fs_hint: Option<&'static str>,
}

/// The parsed partition map.
#[derive(Clone, Debug, Serialize)]
pub struct PartitionMap {
    /// The dominant scheme.
    pub scheme: Scheme,
    /// Partitions in source order.
    pub entries: Vec<Partition>,
    /// Confidence 0..1 that the scheme/entries are right.
    pub confidence: f32,
    /// Human-readable evidence / caveats.
    pub notes: Vec<String>,
    /// Detected container anchors (APFS/Core Storage/APM), detect-only.
    pub containers: Vec<Container>,
}

impl PartitionMap {
    /// Windows the planner should hand to the metadata engines: the parsed
    /// partitions, or — when there is no table — the whole source as one volume.
    #[must_use]
    pub fn volume_windows(&self, source_len: u64) -> Vec<(u64, u64)> {
        if self.entries.is_empty() {
            vec![(0, source_len)]
        } else {
            self.entries.iter().map(|p| (p.start, p.len)).collect()
        }
    }
}

/// A detected container/volume anchor (detect-only; parsed in later phases).
#[derive(Clone, Debug, Serialize)]
pub struct Container {
    /// Kind, e.g. `"apfs-container"`, `"core-storage"`, `"apm"`.
    pub kind: &'static str,
    /// Byte offset of the anchor.
    pub offset: u64,
    /// Evidence string.
    pub evidence: String,
}

const MBR_SIG_OFF: usize = 510;
const MBR_TABLE_OFF: usize = 446;

/// Scan `src` for a partition scheme.
#[must_use]
pub fn scan(src: &Arc<dyn BlockSource>) -> PartitionMap {
    let ss = u64::from(src.sector_size().max(512));
    let total = src.len();
    let lba0 = read_bytes(src, 0, 512);

    let mbr = mbr::parse(&lba0, ss, total);
    let gpt = gpt::parse(src, ss, total);

    let mut containers = detect_containers(src, ss, total);

    match gpt {
        Some(mut g) => {
            // A GPT disk normally carries a protective MBR (one 0xEE entry).
            // If the MBR also maps real partitions, it's a hybrid MBR.
            let real_mbr: Vec<Partition> = mbr
                .as_ref()
                .map(|m| {
                    m.entries
                        .iter()
                        .filter(|p| p.type_byte != Some(0xEE))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if !real_mbr.is_empty() {
                g.scheme = Scheme::HybridMbr;
                for mut p in real_mbr {
                    p.scheme = Scheme::Mbr;
                    g.entries.push(p);
                }
                g.notes.push(format!(
                    "hybrid MBR: {} non-protective MBR entry(ies) alongside the GPT",
                    g.entries.iter().filter(|p| p.scheme == Scheme::Mbr).count()
                ));
            }
            g.containers.append(&mut containers);
            g
        }
        None => match mbr {
            Some(mut m) if !m.entries.is_empty() => {
                mbr::expand(src, &lba0, total, &mut m);
                m.containers.append(&mut containers);
                m
            }
            _ => {
                // No GPT, no usable MBR. Maybe APM, else a bare filesystem.
                if let Some(apm) = apm::detect(src, ss) {
                    return PartitionMap {
                        scheme: Scheme::Apm,
                        entries: Vec::new(),
                        confidence: 0.6,
                        notes: vec![apm],
                        containers,
                    };
                }
                PartitionMap {
                    scheme: Scheme::None,
                    entries: Vec::new(),
                    confidence: 0.5,
                    notes: vec![
                        "no GPT or MBR partition table; treating the whole source as one volume"
                            .to_string(),
                    ],
                    containers,
                }
            }
        },
    }
}

fn detect_containers(src: &Arc<dyn BlockSource>, _ss: u64, total: u64) -> Vec<Container> {
    let mut out = Vec::new();
    // APFS: nx_superblock magic "NXSB" at offset 32 of block 0 (after the
    // 32-byte obj_phys_t header). Detect-only; parsed in Phase 3.
    let head = read_bytes(src, 0, 64);
    if head.get(32..36) == Some(b"NXSB") {
        out.push(Container {
            kind: "apfs-container",
            offset: 0,
            evidence: "NXSB magic at block 0+32".to_string(),
        });
    }
    // Core Storage: CoreStorageHeader magic "CS" at offset 88 of the PV start
    // is version-dependent; detect-only via the 2-byte signature best-effort.
    if total > 0 && head.get(88..90) == Some(b"CS") {
        out.push(Container {
            kind: "core-storage",
            offset: 0,
            evidence: "Core Storage signature at block 0+88".to_string(),
        });
    }
    out
}

/// Read `len` bytes at absolute `offset`, sector-aligning the underlying read
/// and zero-filling anything unavailable. Offsets/lengths are clamped to the
/// source.
pub(crate) fn read_bytes(src: &Arc<dyn BlockSource>, offset: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let total = src.len();
    if len == 0 || offset >= total {
        return out;
    }
    let ss = u64::from(src.sector_size().max(1));
    let aligned_start = offset - (offset % ss);
    let end = offset.saturating_add(len as u64).min(total);
    let aligned_end = end.div_ceil(ss).saturating_mul(ss).min(align_up(total, ss));
    if aligned_end <= aligned_start {
        return out;
    }
    let span = (aligned_end - aligned_start) as usize;
    let mut buf = vec![0u8; span];
    let _ = src.read_at(aligned_start, &mut buf);
    let skip = (offset - aligned_start) as usize;
    let avail = span.saturating_sub(skip);
    let copy = len.min(avail).min((end - offset) as usize);
    if let (Some(dst), Some(s)) = (out.get_mut(..copy), buf.get(skip..skip + copy)) {
        dst.copy_from_slice(s);
    }
    out
}

fn align_up(v: u64, a: u64) -> u64 {
    match v % a {
        0 => v,
        r => v.saturating_add(a - r),
    }
}

/// Little-endian readers used across the MBR/GPT parsers.
pub(crate) fn le_u32(b: &[u8], off: usize) -> u32 {
    match b.get(off..off + 4) {
        Some(s) => {
            let mut a = [0u8; 4];
            a.copy_from_slice(s);
            u32::from_le_bytes(a)
        }
        None => 0,
    }
}
pub(crate) fn le_u64(b: &[u8], off: usize) -> u64 {
    match b.get(off..off + 8) {
        Some(s) => {
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            u64::from_le_bytes(a)
        }
        None => 0,
    }
}

/// Coarse filesystem hint from an MBR partition type byte.
pub(crate) fn mbr_fs_hint(t: u8) -> Option<&'static str> {
    match t {
        0x01 | 0x04 | 0x06 | 0x0E => Some("fat"),
        0x0B | 0x0C => Some("fat32"),
        0x07 => Some("exfat/ntfs"),
        0xAF => Some("hfs+"),
        0x83 => Some("linux"),
        0xEE => Some("gpt-protective"),
        _ => None,
    }
}

mod apm;
mod gpt;
mod mbr;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use reclaim_block::ImageFile;
    use std::io::Write;

    fn src_from(bytes: &[u8]) -> Arc<dyn BlockSource> {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        // Leak the tempfile handle for the test's lifetime via persist to a path
        // kept by the returned source's open; simpler: open then keep file alive.
        let path = f.path().to_path_buf();
        let src: Arc<dyn BlockSource> = Arc::new(ImageFile::open(&path).unwrap());
        std::mem::forget(f); // keep the backing file until process exit (test only)
        src
    }

    #[test]
    fn no_table_is_scheme_none() {
        let mut img = vec![0u8; 1024 * 1024];
        img[0..4].copy_from_slice(b"test");
        let src = src_from(&img);
        let map = scan(&src);
        assert_eq!(map.scheme, Scheme::None);
        assert_eq!(map.volume_windows(src.len()), vec![(0, src.len())]);
    }
}
