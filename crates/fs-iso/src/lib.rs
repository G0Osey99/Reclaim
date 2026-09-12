//! Reclaim ISO 9660 / Joliet / UDF read-only listing engine
//! (docs/plan/04 §2 ISO row).
//!
//! Implemented from ECMA-119 (ISO 9660), the Joliet specification and
//! ECMA-167/UDF (build guide Part 1.4 rule 2 — primary specs, no GPL). Optical
//! media has no deletion, so every entry is `Live`; the value is listing files
//! out of a damaged or partially-written image and providing exact extents.
//!
//! * **ISO 9660**: Primary Volume Descriptor at sector 16, directory-record
//!   tree walk. Names are `NAME.EXT;version`.
//! * **Joliet**: a Supplementary VD with a UCS-2 escape sequence; its directory
//!   tree carries the real (Unicode) names and is preferred when present.
//! * **UDF**: detected via the Anchor Volume Descriptor Pointer at sector 256;
//!   a full UDF file-entry walk is a later item, so UDF images are probed and,
//!   when they also carry an ISO 9660 tree (the common hybrid), listed via that.
//!
//! Reads a whole-source volume (ISO images are super-floppies — one volume).
//! Parses hostile bytes; denies panicking accessors (build guide Part 1.4 rule 3).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

use reclaim_block::BlockSource;
use reclaim_fs_core::{
    Entry, EntryKind, EntrySink, EntryState, Extent, FileSystem, FsError, Probe, WalkOpts,
    WalkStats,
};
use std::sync::Arc;

const SECTOR: u64 = 2048;
const VD_START_SECTOR: u64 = 16;
const CD001: &[u8; 5] = b"CD001";
/// Cap on directory records + recursion (DoS guard on crafted images).
const MAX_ENTRIES: usize = 5_000_000;
const MAX_DEPTH: u32 = 64;

/// An opened ISO 9660 / Joliet volume.
pub struct Iso {
    src: Arc<dyn BlockSource>,
    block_size: u64,
    /// Root directory record (extent LBA, data length) — Joliet preferred.
    root_lba: u64,
    root_len: u64,
    joliet: bool,
    label: Option<String>,
    is_udf: bool,
}

impl std::fmt::Debug for Iso {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Iso")
            .field("block_size", &self.block_size)
            .field("joliet", &self.joliet)
            .field("udf", &self.is_udf)
            .field("root_lba", &self.root_lba)
            .finish()
    }
}

/// Probe a source for ISO 9660 / Joliet / UDF.
#[must_use]
pub fn probe(src: &Arc<dyn BlockSource>) -> Option<Probe> {
    let has_iso = read(src, VD_START_SECTOR * SECTOR + 1, 5) == CD001;
    let has_udf = detect_udf(src);
    if !has_iso && !has_udf {
        return None;
    }
    let (label, block_size) = if has_iso {
        let pvd = read(src, VD_START_SECTOR * SECTOR, SECTOR as usize);
        (
            read_strd(&pvd, 40, 32),
            both_u16(&pvd, 128).max(2048) as u64,
        )
    } else {
        (None, SECTOR)
    };
    let kind = if has_udf && !has_iso {
        "udf"
    } else if has_udf {
        "iso9660+udf"
    } else {
        "iso9660"
    };
    Some(Probe {
        fs_kind: kind,
        confidence: if has_iso { 0.97 } else { 0.9 },
        block_size: u32::try_from(block_size).unwrap_or(2048),
        label,
        uuid: None,
        total_bytes: src.len(),
    })
}

/// Open an ISO volume.
pub fn open(src: Arc<dyn BlockSource>, _probe: Probe) -> Result<Iso, FsError> {
    let has_iso = read(&src, VD_START_SECTOR * SECTOR + 1, 5) == CD001;
    let is_udf = detect_udf(&src);
    if !has_iso {
        if is_udf {
            // UDF-only: expose the volume as detect-only (walk yields nothing).
            return Ok(Iso {
                src,
                block_size: SECTOR,
                root_lba: 0,
                root_len: 0,
                joliet: false,
                label: None,
                is_udf: true,
            });
        }
        return Err(FsError::NotThisFs(
            "no CD001 primary volume descriptor".into(),
        ));
    }

    // Scan volume descriptors: PVD (type 1) + any Joliet SVD (type 2).
    let mut pvd_root = None;
    let mut joliet_root = None;
    let mut block_size = SECTOR;
    let mut label = None;
    for i in 0..32u64 {
        let off = (VD_START_SECTOR + i) * SECTOR;
        let vd = read(&src, off, SECTOR as usize);
        if vd.get(1..6) != Some(CD001) {
            break;
        }
        match vd.first().copied() {
            Some(1) => {
                block_size = both_u16(&vd, 128).max(2048) as u64;
                label = read_strd(&vd, 40, 32);
                pvd_root = Some(root_record(&vd));
            }
            Some(2) if is_joliet(&vd) => {
                joliet_root = Some(root_record(&vd));
            }
            Some(255) => break, // volume descriptor set terminator
            _ => {}
        }
    }

    let (joliet, (root_lba, root_len)) = match joliet_root {
        Some(r) => (true, r),
        None => (
            false,
            pvd_root.ok_or_else(|| FsError::Corrupt("iso: no root directory record".into()))?,
        ),
    };
    if root_lba == 0 {
        return Err(FsError::Corrupt("iso: zero root extent".into()));
    }

    Ok(Iso {
        src,
        block_size,
        root_lba,
        root_len,
        joliet,
        label,
        is_udf,
    })
}

/// Boxed [`open`] for the engine registry.
pub fn open_boxed(src: Arc<dyn BlockSource>, probe: Probe) -> Result<Box<dyn FileSystem>, FsError> {
    Ok(Box::new(open(src, probe)?))
}

impl FileSystem for Iso {
    fn kind(&self) -> &'static str {
        if self.joliet {
            "joliet"
        } else if self.is_udf && self.root_lba == 0 {
            "udf"
        } else {
            "iso9660"
        }
    }

    fn block_size(&self) -> u32 {
        u32::try_from(self.block_size).unwrap_or(2048)
    }

    fn allocation_bitmap(&self) -> Option<reclaim_fs_core::Bitmap> {
        None // optical media: everything written is "allocated"; carve handles it
    }

    fn walk(&self, sink: &mut dyn EntrySink, opts: &WalkOpts) -> Result<WalkStats, FsError> {
        let mut stats = WalkStats::default();
        if !opts.include_live || self.root_lba == 0 {
            return Ok(stats);
        }
        let mut next_id = 1u64;
        self.walk_dir(
            self.root_lba,
            self.root_len,
            "",
            0,
            &mut next_id,
            sink,
            &mut stats,
            opts,
        );
        Ok(stats)
    }
}

impl Iso {
    /// The volume label from the primary volume descriptor, if any.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_dir(
        &self,
        lba: u64,
        len: u64,
        path: &str,
        depth: u32,
        next_id: &mut u64,
        sink: &mut dyn EntrySink,
        stats: &mut WalkStats,
        opts: &WalkOpts,
    ) {
        if depth > MAX_DEPTH || (stats.emitted as usize) >= opts.max_entries.min(MAX_ENTRIES) {
            return;
        }
        let data = read(&self.src, lba.saturating_mul(self.block_size), len as usize);
        let mut pos = 0usize;
        let mut guard = 0usize;
        // Collect subdirs to recurse after finishing this block set.
        let mut subdirs: Vec<(u64, u64, String)> = Vec::new();
        while pos < data.len() {
            guard += 1;
            if guard > MAX_ENTRIES {
                break;
            }
            let reclen = data.get(pos).copied().unwrap_or(0) as usize;
            if reclen == 0 {
                // Padding to the next logical sector.
                let next = (pos / self.block_size as usize + 1) * self.block_size as usize;
                if next <= pos {
                    break;
                }
                pos = next;
                continue;
            }
            if pos + reclen > data.len() {
                break;
            }
            let Some(rec) = data.get(pos..pos + reclen) else {
                break;
            };
            let ext_lba = u64::from(both_u32(rec, 2));
            let ext_len = u64::from(both_u32(rec, 10));
            let flags = rec.get(25).copied().unwrap_or(0);
            let name_len = rec.get(32).copied().unwrap_or(0) as usize;
            let name_bytes = rec.get(33..33 + name_len).unwrap_or(&[]);
            pos += reclen;

            // Skip the "." (0x00) and ".." (0x01) self/parent records.
            if name_len == 1 && (name_bytes == [0u8] || name_bytes == [1u8]) {
                continue;
            }
            let is_dir = flags & 0x02 != 0;
            let name = self.decode_name(name_bytes, is_dir);
            if name.is_empty() {
                continue;
            }
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            let id = *next_id;
            *next_id += 1;

            if is_dir {
                if ext_lba != 0 && ext_len != 0 {
                    subdirs.push((ext_lba, ext_len, child_path.clone()));
                }
            } else {
                let mut entry =
                    Entry::new_file(id, name, name_bytes.to_vec(), ext_len, EntryState::Live);
                entry.kind = EntryKind::File;
                entry.path = Some(child_path);
                if ext_lba != 0 && ext_len != 0 {
                    entry.extents = vec![Extent {
                        offset: ext_lba.saturating_mul(self.block_size),
                        len: ext_len,
                    }];
                }
                entry.confidence = 1.0;
                stats.emitted += 1;
                stats.live += 1;
                sink.emit(entry);
            }
            if (stats.emitted as usize) >= opts.max_entries.min(MAX_ENTRIES) {
                return;
            }
        }
        for (l, ln, p) in subdirs {
            self.walk_dir(l, ln, &p, depth + 1, next_id, sink, stats, opts);
        }
    }

    /// Decode a directory-record identifier into a display name.
    fn decode_name(&self, raw: &[u8], is_dir: bool) -> String {
        if self.joliet {
            // UCS-2 big-endian.
            let mut units = Vec::new();
            let mut i = 0;
            while i + 1 < raw.len() {
                let hi = raw.get(i).copied().unwrap_or(0);
                let lo = raw.get(i + 1).copied().unwrap_or(0);
                units.push(u16::from_be_bytes([hi, lo]));
                i += 2;
            }
            let s = String::from_utf16_lossy(&units);
            strip_version(&s, is_dir)
        } else {
            strip_version(&String::from_utf8_lossy(raw), is_dir)
        }
    }
}

/// Strip the ISO 9660 `;version` suffix (and a trailing `.` on bare names).
fn strip_version(name: &str, is_dir: bool) -> String {
    if is_dir {
        return name.to_string();
    }
    let base = name.split(';').next().unwrap_or(name);
    base.strip_suffix('.').unwrap_or(base).to_string()
}

/// A volume descriptor's root directory record → (extent LBA, data length).
fn root_record(vd: &[u8]) -> (u64, u64) {
    // Root directory record occupies 34 bytes at offset 156.
    let rec = vd.get(156..156 + 34).unwrap_or(&[]);
    (u64::from(both_u32(rec, 2)), u64::from(both_u32(rec, 10)))
}

/// True if a type-2 SVD carries a Joliet UCS-2 escape sequence at offset 88.
fn is_joliet(vd: &[u8]) -> bool {
    let esc = vd.get(88..88 + 3).unwrap_or(&[]);
    esc == b"%/@" || esc == b"%/C" || esc == b"%/E"
}

/// Detect a UDF volume via the Anchor Volume Descriptor Pointer at sector 256
/// (tag identifier 2).
fn detect_udf(src: &Arc<dyn BlockSource>) -> bool {
    let avdp = read(src, 256 * SECTOR, 16);
    // Descriptor tag: tag identifier (LE u16) at offset 0 == 2 (AVDP).
    let tag = u16::from_le_bytes([
        avdp.first().copied().unwrap_or(0),
        avdp.get(1).copied().unwrap_or(0),
    ]);
    if tag == 2 {
        return true;
    }
    // Or an "NSR02"/"NSR03" descriptor in the volume-recognition sequence
    // (sectors 16..19, identifier at byte offset 1).
    for sec in 16..20u64 {
        let vrs = read(src, sec * SECTOR + 1, 5);
        if vrs.starts_with(b"NSR0") {
            return true;
        }
    }
    false
}

// ------------------------ helpers ------------------------

fn read(src: &Arc<dyn BlockSource>, offset: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let total = src.len();
    if len == 0 || offset >= total {
        return out;
    }
    let ss = u64::from(src.sector_size().max(1));
    let aligned = offset - (offset % ss);
    let end = offset.saturating_add(len as u64).min(total);
    let aligned_end = end.div_ceil(ss).saturating_mul(ss);
    let span = (aligned_end - aligned) as usize;
    let mut buf = vec![0u8; span];
    let _ = src.read_at(aligned, &mut buf);
    let skip = (offset - aligned) as usize;
    let avail = span.saturating_sub(skip);
    let copy = len.min(avail).min((end - offset) as usize);
    if let (Some(d), Some(s)) = (out.get_mut(..copy), buf.get(skip..skip + copy)) {
        d.copy_from_slice(s);
    }
    out
}

/// Both-endian u16 (ISO 9660 stores LE then BE); read the LE half.
fn both_u16(b: &[u8], off: usize) -> u16 {
    b.get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .unwrap_or(0)
}

/// Both-endian u32; read the LE half.
fn both_u32(b: &[u8], off: usize) -> u32 {
    b.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .unwrap_or(0)
}

/// Read a space-padded d-characters string field.
fn read_strd(b: &[u8], off: usize, len: usize) -> Option<String> {
    let s = b.get(off..off + len)?;
    let t = String::from_utf8_lossy(s).trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}
