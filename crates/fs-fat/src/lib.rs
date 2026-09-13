//! Reclaim FAT12/16/32 metadata engine (docs/plan/04 §2 FAT row, §3.4).
//!
//! Implemented from the public **Microsoft FAT specification**. The engine:
//!
//! * parses the BPB and classifies FAT12/16/32 by cluster count, and reads the
//!   backup boot sector (FAT32, offset 50) when the primary looks wrong,
//! * walks the root directory (fixed region on FAT12/16, cluster chain on
//!   FAT32) and subdirectories, reconstructing long names from LFN (0x0F)
//!   entries and decoding lowercase-flagged 8.3 names,
//! * recovers deleted entries (first byte 0xE5): the long name survives intact
//!   in the LFN char payload; for 8.3-only names whose first character is lost,
//!   it is inferred from the common prefix of live sibling names,
//! * resolves extents: live files follow the FAT chain; deleted files' chains
//!   are freed on delete, so the engine assumes contiguous from the first
//!   cluster and marks the result `Suspect` (docs/plan/04 §3.4). On FAT32 the
//!   high 16 bits of the first cluster may be zeroed by Windows on delete — a
//!   header-matching heuristic searches candidate clusters,
//! * compares the two FATs and lowers confidence on a mismatch.
//!
//! Parses hostile bytes; denies panicking accessors (build guide Part 1.4 rule 3).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

use reclaim_block::BlockSource;
use reclaim_fs_core::{
    Bitmap, Entry, EntryKind, EntrySink, EntryState, Extent, FileSystem, FsError, Probe, WalkOpts,
    WalkStats,
};
use std::sync::Arc;

const DELETED: u8 = 0xE5;
const ATTR_LFN: u8 = 0x0F;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_VOLUME_ID: u8 = 0x08;

/// FAT width.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FatKind {
    /// FAT12.
    Fat12,
    /// FAT16.
    Fat16,
    /// FAT32.
    Fat32,
}

impl FatKind {
    fn label(self) -> &'static str {
        match self {
            FatKind::Fat12 => "fat12",
            FatKind::Fat16 => "fat16",
            FatKind::Fat32 => "fat32",
        }
    }
    fn eoc(self) -> u32 {
        match self {
            FatKind::Fat12 => 0x0FF8,
            FatKind::Fat16 => 0xFFF8,
            FatKind::Fat32 => 0x0FFF_FFF8,
        }
    }
    fn mask(self) -> u32 {
        match self {
            FatKind::Fat12 => 0x0FFF,
            FatKind::Fat16 => 0xFFFF,
            FatKind::Fat32 => 0x0FFF_FFFF,
        }
    }
}

/// An opened FAT volume.
pub struct FatFs {
    src: Arc<dyn BlockSource>,
    kind: FatKind,
    bytes_per_sector: u32,
    cluster_size: u64,
    fat_offset: u64,
    fat_size_bytes: u64,
    num_fats: u8,
    first_data_sector: u64,
    count_of_clusters: u32,
    root_region_off: u64, // FAT12/16 fixed root dir byte offset
    root_region_len: u64, // FAT12/16 fixed root dir byte length
    root_cluster: u32,    // FAT32
    total_bytes: u64,
    serial: u32,
    fats_match: bool,
    fat0: Vec<u8>, // cached FAT #0 (bounded)
}

impl std::fmt::Debug for FatFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FatFs")
            .field("kind", &self.kind)
            .field("cluster_size", &self.cluster_size)
            .field("count_of_clusters", &self.count_of_clusters)
            .field("fats_match", &self.fats_match)
            .finish()
    }
}

struct Bpb {
    bytes_per_sector: u32,
    sectors_per_cluster: u32,
    reserved_sectors: u32,
    num_fats: u8,
    root_entries: u32,
    total_sectors: u64,
    fat_size: u64,
    root_cluster: u32,
    serial: u32,
}

fn parse_bpb(boot: &[u8]) -> Option<Bpb> {
    let bytes_per_sector = u32::from(le_u16(boot, 11));
    let sectors_per_cluster = u32::from(boot.get(13).copied().unwrap_or(0));
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) {
        return None;
    }
    if !sectors_per_cluster.is_power_of_two() || sectors_per_cluster == 0 {
        return None;
    }
    let reserved_sectors = u32::from(le_u16(boot, 14));
    if reserved_sectors == 0 {
        return None;
    }
    let num_fats = boot.get(16).copied().unwrap_or(0);
    if num_fats == 0 || num_fats > 2 {
        return None;
    }
    let root_entries = u32::from(le_u16(boot, 17));
    let tot16 = u32::from(le_u16(boot, 19));
    let tot32 = le_u32(boot, 32);
    let total_sectors = if tot16 != 0 {
        u64::from(tot16)
    } else {
        u64::from(tot32)
    };
    let fat16 = u32::from(le_u16(boot, 22));
    let fat32 = le_u32(boot, 36);
    let fat_size = if fat16 != 0 {
        u64::from(fat16)
    } else {
        u64::from(fat32)
    };
    if total_sectors == 0 || fat_size == 0 {
        return None;
    }
    let root_cluster = le_u32(boot, 44);
    // Volume serial lives at 39 (FAT12/16) or 67 (FAT32).
    let serial = if fat16 != 0 {
        le_u32(boot, 39)
    } else {
        le_u32(boot, 67)
    };
    Some(Bpb {
        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        num_fats,
        root_entries,
        total_sectors,
        fat_size,
        root_cluster,
        serial,
    })
}

/// Probe a volume source for FAT (docs/plan/04 §2).
#[must_use]
pub fn probe(src: &Arc<dyn BlockSource>) -> Option<Probe> {
    let boot = read(src, 0, 512);
    if boot.get(510..512) != Some(&[0x55, 0xAA]) {
        return None;
    }
    let bpb = parse_bpb(&boot)?;
    let (kind, count) = classify(&bpb);
    // exFAT shares nothing here; NTFS has "NTFS    " at +3 — exclude it.
    if boot.get(3..11) == Some(b"NTFS    ") || boot.get(3..11) == Some(b"EXFAT   ") {
        return None;
    }
    if count == 0 {
        return None;
    }
    let cluster_size = u64::from(bpb.bytes_per_sector) * u64::from(bpb.sectors_per_cluster);
    Some(Probe {
        fs_kind: match kind {
            FatKind::Fat12 => "fat12",
            FatKind::Fat16 => "fat16",
            FatKind::Fat32 => "fat32",
        },
        confidence: 0.9,
        block_size: u32::try_from(cluster_size).unwrap_or(0),
        label: read_label(&boot, kind),
        uuid: Some(format!("{:08X}", bpb.serial)),
        total_bytes: bpb
            .total_sectors
            .saturating_mul(u64::from(bpb.bytes_per_sector)),
    })
}

fn classify(bpb: &Bpb) -> (FatKind, u32) {
    let root_dir_sectors = (bpb.root_entries * 32).div_ceil(bpb.bytes_per_sector);
    let first_data_sector = bpb.reserved_sectors as u64
        + u64::from(bpb.num_fats) * bpb.fat_size
        + u64::from(root_dir_sectors);
    let data_sectors = bpb.total_sectors.saturating_sub(first_data_sector);
    let count = (data_sectors / u64::from(bpb.sectors_per_cluster)) as u32;
    let kind = if count < 4085 {
        FatKind::Fat12
    } else if count < 65525 {
        FatKind::Fat16
    } else {
        FatKind::Fat32
    };
    (kind, count)
}

/// Open a FAT volume.
pub fn open(src: Arc<dyn BlockSource>, _probe: Probe) -> Result<FatFs, FsError> {
    let boot = read(&src, 0, 512);
    let bpb = match parse_bpb(&boot) {
        Some(b) if boot.get(510..512) == Some(&[0x55, 0xAA]) => b,
        _ => {
            // Try the FAT32 backup boot sector at offset 50 (sector number).
            let bk = u64::from(le_u16(&boot, 50));
            if bk == 0 {
                return Err(FsError::NotThisFs("no valid FAT BPB".into()));
            }
            let bps = u32::from(le_u16(&boot, 11)).max(512);
            let bbuf = read(&src, bk * u64::from(bps), 512);
            parse_bpb(&bbuf).ok_or_else(|| FsError::Corrupt("FAT BPB + backup both bad".into()))?
        }
    };

    let (kind, count_of_clusters) = classify(&bpb);
    let bytes_per_sector = bpb.bytes_per_sector;
    let cluster_size = u64::from(bytes_per_sector) * u64::from(bpb.sectors_per_cluster);
    let root_dir_sectors = (bpb.root_entries * 32).div_ceil(bytes_per_sector);
    let fat_offset = bpb.reserved_sectors as u64 * u64::from(bytes_per_sector);
    let fat_size_bytes = bpb.fat_size * u64::from(bytes_per_sector);
    let first_data_sector = bpb.reserved_sectors as u64
        + u64::from(bpb.num_fats) * bpb.fat_size
        + u64::from(root_dir_sectors);
    let root_region_off = (bpb.reserved_sectors as u64 + u64::from(bpb.num_fats) * bpb.fat_size)
        * u64::from(bytes_per_sector);
    let root_region_len = u64::from(root_dir_sectors) * u64::from(bytes_per_sector);
    let total_bytes = bpb.total_sectors * u64::from(bytes_per_sector);

    // Cache FAT #0 (bounded to 64 MiB) for chain walking and the bitmap.
    let fat0 = read(
        &src,
        fat_offset,
        fat_size_bytes.min(64 * 1024 * 1024) as usize,
    );
    // Compare the two FATs (sample).
    let fats_match = if bpb.num_fats >= 2 {
        let fat1 = read(
            &src,
            fat_offset + fat_size_bytes,
            fat_size_bytes.min(1024 * 1024) as usize,
        );
        let n = fat1.len().min(fat0.len());
        fat0.get(..n) == fat1.get(..n)
    } else {
        true
    };

    Ok(FatFs {
        src,
        kind,
        bytes_per_sector,
        cluster_size,
        fat_offset,
        fat_size_bytes,
        num_fats: bpb.num_fats,
        first_data_sector,
        count_of_clusters,
        root_region_off,
        root_region_len,
        root_cluster: bpb.root_cluster,
        total_bytes,
        serial: bpb.serial,
        fats_match,
        fat0,
    })
}

/// Boxed [`open`] for the engine registry.
pub fn open_boxed(src: Arc<dyn BlockSource>, probe: Probe) -> Result<Box<dyn FileSystem>, FsError> {
    Ok(Box::new(open(src, probe)?))
}

impl FatFs {
    fn cluster_offset(&self, cluster: u32) -> Option<u64> {
        if cluster < 2 || cluster >= self.count_of_clusters.saturating_add(2) {
            return None;
        }
        Some(
            (self.first_data_sector
                + u64::from(cluster - 2) * (self.cluster_size / u64::from(self.bytes_per_sector)))
                * u64::from(self.bytes_per_sector),
        )
    }

    /// Read the FAT entry for `cluster` from the cached FAT #0.
    fn fat_entry(&self, cluster: u32) -> u32 {
        match self.kind {
            FatKind::Fat32 => le_u32(&self.fat0, cluster as usize * 4) & self.kind.mask(),
            FatKind::Fat16 => u32::from(le_u16(&self.fat0, cluster as usize * 2)),
            FatKind::Fat12 => {
                let idx = cluster as usize + cluster as usize / 2;
                let lo = self.fat0.get(idx).copied().unwrap_or(0) as u32;
                let hi = self.fat0.get(idx + 1).copied().unwrap_or(0) as u32;
                let v = lo | (hi << 8);
                if cluster & 1 == 0 {
                    v & 0x0FFF
                } else {
                    (v >> 4) & 0x0FFF
                }
            }
        }
    }

    /// Follow a cluster chain from `first`.
    fn chain(&self, first: u32, max: u64) -> Vec<u32> {
        let mut out = Vec::new();
        let mut cur = first;
        let cap = max.min(u64::from(self.count_of_clusters) + 2).max(1);
        let mut steps = 0u64;
        while cur >= 2 && cur < self.count_of_clusters + 2 && steps < cap {
            out.push(cur);
            steps += 1;
            let next = self.fat_entry(cur);
            if next >= self.kind.eoc()
                || next < 2
                || next >= self.count_of_clusters + 2
                || next == cur
            {
                break;
            }
            cur = next;
        }
        out
    }

    /// Build an allocation bitmap from the FAT (cluster allocated iff FAT != 0).
    fn fat_bitmap(&self) -> Option<Bitmap> {
        let n = self.count_of_clusters;
        if n == 0 {
            return None;
        }
        let bytes = ((n as u64 + 2).div_ceil(8)) as usize;
        let mut bits = vec![0u8; bytes.min(16 * 1024 * 1024)];
        for c in 2..n + 2 {
            if self.fat_entry(c) != 0 {
                let bit_index = c; // bit `c` represents cluster `c`; bit 0 covers cluster 0.
                if let Some(byte) = bits.get_mut((bit_index / 8) as usize) {
                    *byte |= 1 << (bit_index % 8);
                }
            }
        }
        // base_offset is the byte offset of cluster 0 in this bit space: cluster
        // `c` sits at data region. We anchor bit 0 at the byte offset that cluster
        // 0 *would* occupy so is_offset_allocated maps cleanly.
        let cluster0_off = self
            .cluster_offset(2)
            .map(|o| o.saturating_sub(2 * self.cluster_size))
            .unwrap_or(0);
        Some(Bitmap::new(
            u32::try_from(self.cluster_size).unwrap_or(0),
            cluster0_off,
            u64::from(n) + 2,
            bits,
        ))
    }

    /// Read a directory's raw bytes: the fixed region for the FAT12/16 root, or
    /// the cluster chain otherwise.
    fn read_directory(&self, first_cluster: u32, is_fixed_root: bool) -> Vec<u8> {
        if is_fixed_root {
            return read(
                &self.src,
                self.root_region_off,
                self.root_region_len.min(16 * 1024 * 1024) as usize,
            );
        }
        let clusters = self.chain(first_cluster, 1 << 20);
        let mut buf = Vec::new();
        for c in clusters {
            let Some(off) = self.cluster_offset(c) else {
                break;
            };
            buf.extend_from_slice(&read(&self.src, off, self.cluster_size as usize));
            if buf.len() as u64 > 256 * 1024 * 1024 {
                break;
            }
        }
        buf
    }

    /// Resolve extents for a file. Live → FAT chain; deleted → contiguous
    /// assumption (the chain is freed on delete).
    fn resolve_extents(&self, first: u32, size: u64, deleted: bool) -> (Vec<Extent>, bool) {
        if size == 0 || first < 2 {
            return (Vec::new(), false);
        }
        let need = size.div_ceil(self.cluster_size);
        if !deleted {
            let clusters = self.chain(first, need.saturating_add(2));
            if !clusters.is_empty() {
                let ex = coalesce(&clusters, self, size);
                if !ex.is_empty() {
                    return (ex, false);
                }
            }
        }
        // Contiguous assumption (deleted, or a broken live chain). A file that
        // fits in a single cluster (`need <= 1`) occupies exactly its first
        // cluster — contiguity is then a certainty, not an assumption, so it is
        // `Full`, not `Suspect`. Only multi-cluster files carry the freed-chain
        // uncertainty (docs/plan/04 §3.4).
        let assumed = need > 1;
        match self.cluster_offset(first) {
            Some(start) => {
                let span = need.saturating_mul(self.cluster_size);
                let avail = self.total_bytes.saturating_sub(start);
                (
                    vec![Extent {
                        offset: start,
                        len: size.min(span).min(avail),
                    }],
                    assumed,
                )
            }
            None => (Vec::new(), assumed),
        }
    }

    /// FAT32 first-cluster heuristic: when the high word was zeroed on delete,
    /// try candidate clusters `lo | (k<<16)` and keep the first whose start
    /// looks non-empty (docs/plan/04 §3.4). Best-effort; returns `first` as-is
    /// when nothing better is found.
    fn repair_first_cluster(&self, hi: u16, lo: u16) -> u32 {
        let first = (u32::from(hi) << 16) | u32::from(lo);
        if self.kind != FatKind::Fat32 || hi != 0 || lo == 0 {
            return first;
        }
        // Only meaningful on volumes with > 64 Ki clusters.
        if self.count_of_clusters <= 0xFFFF {
            return first;
        }
        let mut best = first;
        let mut best_nonzero = 0usize;
        let mut k = 0u32;
        while (u32::from(lo) | (k << 16)) < self.count_of_clusters + 2 && k < 256 {
            let cand = u32::from(lo) | (k << 16);
            if let Some(off) = self.cluster_offset(cand) {
                let head = read(&self.src, off, 16);
                let nz = head.iter().filter(|b| **b != 0).count();
                if nz > best_nonzero {
                    best_nonzero = nz;
                    best = cand;
                }
            }
            k += 1;
        }
        best
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_dir(
        &self,
        first_cluster: u32,
        is_fixed_root: bool,
        parent_path: &str,
        depth: u32,
        sink: &mut dyn EntrySink,
        opts: &WalkOpts,
        bitmap: Option<&Bitmap>,
        stats: &mut WalkStats,
    ) {
        if depth > 64 || stats.emitted as usize >= opts.max_entries {
            return;
        }
        let dir = self.read_directory(first_cluster, is_fixed_root);
        // Pre-scan live SFN names for first-char inference on deleted siblings.
        let siblings = collect_sfn_names(&dir);

        let mut lfn: Vec<Vec<u16>> = Vec::new();
        let mut i = 0usize;
        while i + 32 <= dir.len() {
            if stats.emitted as usize >= opts.max_entries {
                return;
            }
            let Some(ent) = dir.get(i..i + 32) else { break };
            let first_byte = ent.first().copied().unwrap_or(0);
            let attr = ent.get(11).copied().unwrap_or(0);

            if first_byte == 0x00 {
                // End of directory — but slack beyond may hold deleted records
                // on some layouts; we stop, matching on-disk semantics.
                break;
            }
            if attr == ATTR_LFN {
                // Collect LFN char payload (works for live and deleted).
                lfn.push(lfn_chars(ent));
                i += 32;
                continue;
            }
            if attr & ATTR_VOLUME_ID != 0 && attr & ATTR_DIRECTORY == 0 {
                lfn.clear();
                i += 32;
                continue;
            }

            let deleted = first_byte == DELETED;
            let is_dir = attr & ATTR_DIRECTORY != 0;

            // Name: prefer the reconstructed long name, else the 8.3 short name.
            let long = assemble_lfn(&lfn);
            lfn.clear();
            let short = short_name(ent, deleted, &siblings);
            let (name, name_conf) = match long {
                Some(l) if !l.is_empty() => (l, 1.0f32),
                _ => {
                    let c = if deleted && short.starts_with('_') {
                        0.7
                    } else {
                        0.9
                    };
                    (short, c)
                }
            };

            if name == "." || name == ".." || name.is_empty() {
                i += 32;
                continue;
            }

            let hi = le_u16(ent, 20);
            let lo = le_u16(ent, 26);
            let first = if deleted {
                self.repair_first_cluster(hi, lo)
            } else {
                (u32::from(hi) << 16) | u32::from(lo)
            };
            let size = u64::from(le_u32(ent, 28));
            let created = fat_date(le_u16(ent, 16));
            let modified = fat_date(le_u16(ent, 24));

            let include = (deleted && opts.include_deleted) || (!deleted && opts.include_live);
            let child_path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{parent_path}/{name}")
            };

            if is_dir {
                if include {
                    let id = (first as u64) << 20 | (i as u64);
                    let mut e = Entry::new_file(
                        id,
                        name.clone(),
                        name.as_bytes().to_vec(),
                        0,
                        if deleted {
                            EntryState::Deleted
                        } else {
                            EntryState::Live
                        },
                    );
                    e.kind = EntryKind::Dir;
                    e.path = Some(child_path.clone());
                    e.created = created.clone();
                    e.modified = modified.clone();
                    e.confidence = name_conf * 0.9;
                    emit(sink, stats, e);
                }
                // Recurse into live subdirectories only (deleted dir chains are
                // unreliable and risk loops).
                if !deleted && first >= 2 {
                    self.walk_dir(
                        first,
                        false,
                        &child_path,
                        depth + 1,
                        sink,
                        opts,
                        bitmap,
                        stats,
                    );
                }
            } else if include {
                let (extents, assumed) = self.resolve_extents(first, size, deleted);
                let id = (first as u64) << 20 | (i as u64);
                let mut e = Entry::new_file(
                    id,
                    name.clone(),
                    name.as_bytes().to_vec(),
                    size,
                    if deleted {
                        EntryState::Deleted
                    } else {
                        EntryState::Live
                    },
                );
                e.path = Some(child_path);
                e.created = created;
                e.modified = modified;
                e.extents = extents;
                e.contiguous_assumed = assumed;
                e.allocated = bitmap
                    .map(|b| {
                        b.is_offset_allocated(e.extents.first().map(|x| x.offset).unwrap_or(0))
                    })
                    .unwrap_or(false);
                let base = if self.fats_match {
                    name_conf
                } else {
                    name_conf * 0.9
                };
                e.confidence = if assumed { base * 0.8 } else { base };
                emit(sink, stats, e);
            }
            i += 32;
        }
    }
}

fn emit(sink: &mut dyn EntrySink, stats: &mut WalkStats, e: Entry) {
    match e.state {
        EntryState::Live => stats.live += 1,
        _ => stats.deleted += 1,
    }
    stats.emitted += 1;
    sink.emit(e);
}

impl FileSystem for FatFs {
    fn kind(&self) -> &'static str {
        self.kind.label()
    }
    fn block_size(&self) -> u32 {
        u32::try_from(self.cluster_size).unwrap_or(0)
    }
    fn allocation_bitmap(&self) -> Option<Bitmap> {
        self.fat_bitmap()
    }
    fn walk(&self, sink: &mut dyn EntrySink, opts: &WalkOpts) -> Result<WalkStats, FsError> {
        let bitmap = self.fat_bitmap();
        let mut stats = WalkStats::default();
        let fixed_root = self.kind != FatKind::Fat32;
        let root_first = if fixed_root { 0 } else { self.root_cluster };
        self.walk_dir(
            root_first,
            fixed_root,
            "",
            0,
            sink,
            opts,
            bitmap.as_ref(),
            &mut stats,
        );
        let _ = (
            self.fat_offset,
            self.fat_size_bytes,
            self.num_fats,
            self.serial,
        );
        Ok(stats)
    }
}

// ---- directory-entry helpers ----

/// Collect live 8.3 short names (11 raw bytes) for first-char inference.
fn collect_sfn_names(dir: &[u8]) -> Vec<[u8; 11]> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 32 <= dir.len() {
        let Some(ent) = dir.get(i..i + 32) else { break };
        let fb = ent.first().copied().unwrap_or(0);
        let attr = ent.get(11).copied().unwrap_or(0);
        if fb == 0x00 {
            break;
        }
        if attr != ATTR_LFN && fb != DELETED && fb != 0x2E {
            if let Some(n) = ent.get(0..11) {
                let mut a = [0u8; 11];
                a.copy_from_slice(n);
                out.push(a);
            }
        }
        i += 32;
    }
    out
}

/// Decode a short (8.3) name, applying the lowercase NT flags and recovering a
/// deleted first character via sibling common-prefix inference.
fn short_name(ent: &[u8], deleted: bool, siblings: &[[u8; 11]]) -> String {
    let mut name = [0u8; 11];
    if let Some(s) = ent.get(0..11) {
        name.copy_from_slice(s);
    }
    if deleted {
        name[0] = infer_first_char(&name, siblings).unwrap_or(b'_');
    }
    let ntres = ent.get(12).copied().unwrap_or(0);
    let lower_base = ntres & 0x08 != 0;
    let lower_ext = ntres & 0x10 != 0;

    let base_raw = name.get(0..8).unwrap_or(&[]);
    let ext_raw = name.get(8..11).unwrap_or(&[]);
    let base = trim_fat(base_raw, lower_base);
    let ext = trim_fat(ext_raw, lower_ext);
    if ext.is_empty() {
        base
    } else {
        format!("{base}.{ext}")
    }
}

fn trim_fat(raw: &[u8], lower: bool) -> String {
    let mut s: String = raw
        .iter()
        .map(|&b| if b == 0x05 { 0xE5 } else { b } as char)
        .collect();
    let trimmed = s.trim_end().to_string();
    s = if lower {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed
    };
    s
}

/// Infer the lost first character of a deleted 8.3 name from the mode of the
/// first characters of live siblings whose bytes `[1..4]` match.
fn infer_first_char(deleted: &[u8; 11], siblings: &[[u8; 11]]) -> Option<u8> {
    let key = deleted.get(1..4)?;
    let mut counts: Vec<(u8, u32)> = Vec::new();
    for sib in siblings {
        if sib.get(1..4) == Some(key) {
            let c = sib.first().copied().unwrap_or(0);
            if c == 0 || c == b' ' {
                continue;
            }
            match counts.iter_mut().find(|(ch, _)| *ch == c) {
                Some((_, n)) => *n += 1,
                None => counts.push((c, 1)),
            }
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c)
}

/// Extract the 13 UTF-16 units from one LFN entry (positions 1..11, 14..26, 28..32).
fn lfn_chars(ent: &[u8]) -> Vec<u16> {
    let mut v = Vec::with_capacity(13);
    for &range in &[(1usize, 11usize), (14, 26), (28, 32)] {
        let mut p = range.0;
        while p + 1 < range.1 {
            v.push(le_u16(ent, p));
            p += 2;
        }
    }
    v
}

/// Assemble a long name from LFN entries collected in physical order
/// (highest-sequence first), which reversed gives the name left-to-right.
/// Works for deleted entries too (the char payload survives; only the sequence
/// byte is overwritten with 0xE5).
fn assemble_lfn(lfn: &[Vec<u16>]) -> Option<String> {
    if lfn.is_empty() {
        return None;
    }
    let mut units: Vec<u16> = Vec::new();
    for piece in lfn.iter().rev() {
        for &u in piece {
            if u == 0x0000 || u == 0xFFFF {
                continue;
            }
            units.push(u);
        }
    }
    if units.is_empty() {
        return None;
    }
    Some(String::from_utf16_lossy(&units))
}

fn read_label(boot: &[u8], kind: FatKind) -> Option<String> {
    let off = if kind == FatKind::Fat32 { 71 } else { 43 };
    let raw = boot.get(off..off + 11)?;
    let s = String::from_utf8_lossy(raw).trim_end().to_string();
    if s.is_empty() || s == "NO NAME" {
        None
    } else {
        Some(s)
    }
}

/// Coalesce clusters into byte extents, clamped to `size`.
fn coalesce(clusters: &[u32], fs: &FatFs, size: u64) -> Vec<Extent> {
    let mut extents: Vec<Extent> = Vec::new();
    let mut remaining = size;
    let mut run_start: Option<u64> = None;
    let mut run_len: u64 = 0;
    let mut prev: Option<u32> = None;
    for &c in clusters {
        if remaining == 0 {
            break;
        }
        let Some(off) = fs.cluster_offset(c) else {
            break;
        };
        let contiguous = matches!(prev, Some(p) if p + 1 == c);
        if contiguous {
            run_len += fs.cluster_size;
        } else {
            if let Some(s) = run_start {
                extents.push(Extent {
                    offset: s,
                    len: run_len,
                });
            }
            run_start = Some(off);
            run_len = fs.cluster_size;
        }
        remaining = remaining.saturating_sub(remaining.min(fs.cluster_size));
        prev = Some(c);
    }
    if let Some(s) = run_start {
        extents.push(Extent {
            offset: s,
            len: run_len,
        });
    }
    let total: u64 = extents.iter().map(|e| e.len).sum();
    if total > size {
        if let Some(last) = extents.last_mut() {
            last.len = last.len.saturating_sub(total - size);
        }
    }
    extents.retain(|e| e.len > 0);
    extents
}

/// Decode a FAT packed date word to `YYYY-MM-DD`.
fn fat_date(date: u16) -> Option<String> {
    if date == 0 {
        return None;
    }
    let day = date & 0x1F;
    let month = (date >> 5) & 0x0F;
    let year = 1980 + ((date >> 9) & 0x7F) as u32;
    if month == 0 || day == 0 || month > 12 || day > 31 {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

// ---- byte helpers ----

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

fn le_u16(b: &[u8], off: usize) -> u16 {
    match b.get(off..off + 2) {
        Some(s) => u16::from_le_bytes([
            s.first().copied().unwrap_or(0),
            s.get(1).copied().unwrap_or(0),
        ]),
        None => 0,
    }
}
fn le_u32(b: &[u8], off: usize) -> u32 {
    match b.get(off..off + 4) {
        Some(s) => {
            let mut a = [0u8; 4];
            a.copy_from_slice(s);
            u32::from_le_bytes(a)
        }
        None => 0,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn first_char_inference() {
        // Deleted "?ILE_002TXT", siblings "FILE_000JPG"/"FILE_001PNG".
        let del = *b"\xe5ILE_002TXT";
        let sibs = vec![*b"FILE_000JPG", *b"FILE_001PNG"];
        assert_eq!(infer_first_char(&del, &sibs), Some(b'F'));
    }

    #[test]
    fn short_name_lowercase_flag() {
        let mut ent = [0u8; 32];
        ent[0..11].copy_from_slice(b"FILE_000JPG");
        ent[12] = 0x18; // lowercase base + ext
        let s = short_name(&ent, false, &[]);
        assert_eq!(s, "file_000.jpg");
    }

    #[test]
    fn deleted_short_name_recovers_first_char() {
        let mut ent = [0u8; 32];
        ent[0..11].copy_from_slice(b"\xe5ILE_002TXT");
        ent[12] = 0x18;
        let sibs = vec![*b"FILE_000JPG", *b"FILE_001PNG"];
        let s = short_name(&ent, true, &sibs);
        assert_eq!(s, "file_002.txt");
    }

    #[test]
    fn lfn_assembly() {
        // Two pieces physically [seq2="g", seq1="._file_000.jp"] → reversed join.
        let p2: Vec<u16> = "g".encode_utf16().collect();
        let p1: Vec<u16> = "._file_000.jp".encode_utf16().collect();
        let name = assemble_lfn(&[p2, p1]).unwrap();
        assert_eq!(name, "._file_000.jpg");
    }

    #[test]
    fn date_decode() {
        let date: u16 = (46 << 9) | (9 << 5) | 12; // 2026-09-12
        assert_eq!(fat_date(date).as_deref(), Some("2026-09-12"));
    }
}
