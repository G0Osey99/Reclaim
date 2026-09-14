//! Reclaim exFAT metadata engine (docs/plan/04 §2 exFAT row, §3.4).
//!
//! Implemented from the public **Microsoft exFAT file system specification**
//! (build guide Part 1.4 rule 2 — primary specs only, no GPL code). The engine:
//!
//! * parses the boot sector (`EXFAT   ` at +3) and verifies the boot checksum,
//! * loads the allocation bitmap (`$BITMAP`, entry type 0x81) and up-case table
//!   presence (0x82),
//! * walks the directory tree including **deleted entry sets** — an in-use File
//!   entry is 0x85, its Stream 0xC0 and Name 0xC1; when the in-use bit (0x80) is
//!   cleared on delete they become 0x05 / 0x40 / 0x41 but keep name, size and
//!   first cluster (docs/plan/04 §3.4),
//! * resolves extents: `NoFatChain` ⇒ contiguous from the first cluster for
//!   `DataLength`; otherwise it follows the FAT, and when the chain has been
//!   zeroed on delete it assumes contiguous and marks the entry `Suspect`.
//!
//! The engine reads a **volume** source (the partition window); extents are
//! volume-relative and shifted to absolute by the session's `EntrySink`.
//!
//! Parses hostile bytes; denies panicking accessors (build guide Part 1.4 rule 3).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

use reclaim_block::BlockSource;
use reclaim_fs_core::{
    Bitmap, Entry, EntryKind, EntrySink, EntryState, Extent, FileSystem, FsError, Probe, WalkOpts,
    WalkStats,
};
use std::collections::HashSet;
use std::sync::Arc;

/// exFAT directory entry type codes (with the 0x80 in-use bit set).
const TYPE_BITMAP: u8 = 0x81;
const TYPE_UPCASE: u8 = 0x82;
const TYPE_LABEL: u8 = 0x83;
const TYPE_FILE: u8 = 0x85;
const TYPE_STREAM: u8 = 0xC0;
const TYPE_NAME: u8 = 0xC1;

/// Cap on a single directory read (DoS guard on crafted chains).
const MAX_DIR_BYTES: u64 = 32 * 1024 * 1024;

const ATTR_DIRECTORY: u16 = 0x0010;
const FLAG_NO_FAT_CHAIN: u8 = 0x02;

/// An opened exFAT volume.
pub struct ExFat {
    src: Arc<dyn BlockSource>,
    bytes_per_sector: u32,
    cluster_size: u64,
    fat_offset: u64,
    fat_length: u64,
    number_of_fats: u8,
    cluster_heap_offset: u64,
    cluster_count: u32,
    root_dir_cluster: u32,
    volume_length: u64,
    serial: u32,
    checksum_ok: bool,
}

impl std::fmt::Debug for ExFat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExFat")
            .field("cluster_size", &self.cluster_size)
            .field("cluster_count", &self.cluster_count)
            .field("root_dir_cluster", &self.root_dir_cluster)
            .field("checksum_ok", &self.checksum_ok)
            .finish()
    }
}

/// Probe a volume source for exFAT (docs/plan/04 §2).
/// Read the main VBR at offset 0, falling back to the backup boot sector (12
/// logical sectors in) when the main is zeroed/corrupt (docs/plan/04 §3.4). The
/// backup is a byte-identical copy, so its volume-relative geometry is valid for
/// a volume whose start is the source origin.
fn read_boot(src: &Arc<dyn BlockSource>) -> Option<Vec<u8>> {
    let main = read(src, 0, 512);
    if main.get(3..11) == Some(b"EXFAT   ") {
        return Some(main);
    }
    for shift in 9u32..=12 {
        let off = 12u64 << shift; // backup boot sector = 12 * bytes_per_sector
        let bak = read(src, off, 512);
        if bak.get(3..11) == Some(b"EXFAT   ") && bak.get(108).copied() == Some(shift as u8) {
            return Some(bak);
        }
    }
    None
}

#[must_use]
pub fn probe(src: &Arc<dyn BlockSource>) -> Option<Probe> {
    let boot = read_boot(src)?;
    if boot.get(3..11) != Some(b"EXFAT   ") {
        return None;
    }
    let bps_shift = boot.get(108).copied().unwrap_or(0);
    let spc_shift = boot.get(109).copied().unwrap_or(0);
    if !(9..=12).contains(&bps_shift) || u32::from(bps_shift) + u32::from(spc_shift) > 25 {
        return None;
    }
    let bytes_per_sector = 1u64 << bps_shift;
    let cluster_size = bytes_per_sector << spc_shift;
    let volume_length = le_u64(&boot, 72).saturating_mul(bytes_per_sector);
    let serial = le_u32(&boot, 100);
    let checksum_ok = verify_boot_checksum(src, bytes_per_sector as usize);
    Some(Probe {
        fs_kind: "exfat",
        confidence: if checksum_ok { 0.99 } else { 0.85 },
        block_size: u32::try_from(cluster_size).unwrap_or(0),
        label: read_volume_label(src, &boot),
        uuid: Some(format!("{serial:08X}")),
        total_bytes: volume_length,
    })
}

/// Open an exFAT volume (consumes the [`Probe`] from [`probe`]).
pub fn open(src: Arc<dyn BlockSource>, _probe: Probe) -> Result<ExFat, FsError> {
    let boot = read_boot(&src)
        .ok_or_else(|| FsError::NotThisFs("no EXFAT boot signature (main or backup)".into()))?;
    let bps_shift = boot.get(108).copied().unwrap_or(0);
    let spc_shift = boot.get(109).copied().unwrap_or(0);
    if !(9..=12).contains(&bps_shift) || u32::from(bps_shift) + u32::from(spc_shift) > 25 {
        return Err(FsError::Corrupt(
            "implausible exFAT sector/cluster shift".into(),
        ));
    }
    let bytes_per_sector = 1u32 << bps_shift;
    let cluster_size = u64::from(bytes_per_sector) << spc_shift;
    let fat_offset = le_u32(&boot, 80) as u64 * u64::from(bytes_per_sector);
    let fat_length = le_u32(&boot, 84) as u64 * u64::from(bytes_per_sector);
    let cluster_heap_offset = le_u32(&boot, 88) as u64 * u64::from(bytes_per_sector);
    let cluster_count = le_u32(&boot, 92);
    let root_dir_cluster = le_u32(&boot, 96);
    let number_of_fats = boot.get(110).copied().unwrap_or(1).max(1);
    let volume_length = le_u64(&boot, 72).saturating_mul(u64::from(bytes_per_sector));
    let serial = le_u32(&boot, 100);
    let checksum_ok = verify_boot_checksum(&src, bytes_per_sector as usize);

    if cluster_size == 0 || root_dir_cluster < 2 {
        return Err(FsError::Corrupt("exFAT geometry invalid".into()));
    }

    Ok(ExFat {
        src,
        bytes_per_sector,
        cluster_size,
        fat_offset,
        fat_length,
        number_of_fats,
        cluster_heap_offset,
        cluster_count,
        root_dir_cluster,
        volume_length,
        serial,
        checksum_ok,
    })
}

/// Boxed [`open`] for the engine registry.
pub fn open_boxed(src: Arc<dyn BlockSource>, probe: Probe) -> Result<Box<dyn FileSystem>, FsError> {
    Ok(Box::new(open(src, probe)?))
}

impl ExFat {
    fn cluster_offset(&self, cluster: u32) -> Option<u64> {
        if cluster < 2 || cluster >= self.cluster_count.saturating_add(2) {
            return None;
        }
        Some(
            self.cluster_heap_offset
                .saturating_add(u64::from(cluster - 2).saturating_mul(self.cluster_size)),
        )
    }

    /// Read the FAT entry for `cluster` (32-bit exFAT FAT).
    fn fat_next(&self, cluster: u32) -> u32 {
        let off = self.fat_offset + u64::from(cluster) * 4;
        let b = read(&self.src, off, 4);
        le_u32(&b, 0)
    }

    /// Follow the FAT cluster chain from `first`, returning the cluster list.
    /// Bounded by the cluster count with a visited-bit guard against loops.
    fn chain(&self, first: u32, max_clusters: u64) -> Vec<u32> {
        let mut out = Vec::new();
        let mut cur = first;
        let limit = self.cluster_count.saturating_add(2);
        let cap = max_clusters.min(u64::from(limit)).max(1);
        let mut steps = 0u64;
        while cur >= 2 && cur < limit && steps < cap {
            out.push(cur);
            steps += 1;
            let next = self.fat_next(cur);
            // exFAT end-of-chain is 0xFFFFFFFF; 0 / bad / out-of-range stop.
            if next == 0xFFFF_FFFF || next < 2 || next >= limit {
                break;
            }
            if next == cur {
                break;
            }
            cur = next;
        }
        out
    }

    /// Resolve a file's extents given its stream metadata.
    /// Returns `(extents, contiguous_assumed)`.
    fn resolve_extents(
        &self,
        first_cluster: u32,
        data_len: u64,
        no_fat_chain: bool,
        deleted: bool,
    ) -> (Vec<Extent>, bool) {
        if data_len == 0 || first_cluster < 2 {
            return (Vec::new(), false);
        }
        let need_clusters = data_len.div_ceil(self.cluster_size);

        // Contiguous (NoFatChain), or a deleted file whose FAT chain is gone.
        let contiguous = |assumed: bool| -> (Vec<Extent>, bool) {
            match self.cluster_offset(first_cluster) {
                Some(start) => {
                    // Clamp to the heap end.
                    let span = need_clusters.saturating_mul(self.cluster_size);
                    let avail = self.volume_length.saturating_sub(start);
                    (
                        vec![Extent {
                            offset: start,
                            len: data_len.min(span).min(avail),
                        }],
                        assumed,
                    )
                }
                None => (Vec::new(), assumed),
            }
        };

        if no_fat_chain {
            return contiguous(false);
        }

        // Follow the FAT. For a deleted file the entries are usually freed (0);
        // detect that and fall back to a contiguous assumption (Suspect). A file
        // that fits in a single cluster (`need_clusters <= 1`) occupies exactly
        // its first cluster — contiguity is a certainty, not an assumption, so it
        // stays `Full` even when the chain is gone (docs/plan/04 §3.4).
        let assumed = need_clusters > 1;
        let clusters = self.chain(first_cluster, need_clusters.saturating_add(2));
        if clusters.is_empty() || (deleted && (clusters.len() as u64) < need_clusters) {
            return contiguous(assumed);
        }
        // Coalesce consecutive clusters into extents.
        let extents = coalesce(&clusters, self, data_len);
        if extents.is_empty() {
            return contiguous(assumed);
        }
        (extents, false)
    }

    /// True if the root directory carries an up-case table entry (0x82) — the
    /// spec requires one in a healthy volume; its presence is a confidence cue.
    fn has_upcase(&self) -> bool {
        let Some(root) = self.read_directory(self.root_dir_cluster) else {
            return false;
        };
        let mut i = 0usize;
        while i + 32 <= root.len() {
            match root.get(i).copied().unwrap_or(0) {
                TYPE_UPCASE => return true,
                0x00 => break,
                _ => {}
            }
            i += 32;
        }
        false
    }

    /// Load the allocation bitmap from the root directory's `$BITMAP` entry.
    fn load_bitmap(&self) -> Option<Bitmap> {
        let root = self.read_directory(self.root_dir_cluster)?;
        let mut i = 0usize;
        while i + 32 <= root.len() {
            let t = root.get(i).copied().unwrap_or(0);
            if t == TYPE_BITMAP {
                let first = le_u32(&root, i + 20);
                let len = le_u64(&root, i + 24);
                let start = self.cluster_offset(first)?;
                let want = len
                    .min(u64::from(self.cluster_count).div_ceil(8) + 8)
                    .min(64 * 1024 * 1024);
                let bits = read(&self.src, start, want as usize);
                return Some(Bitmap::new(
                    u32::try_from(self.cluster_size).unwrap_or(0),
                    self.cluster_heap_offset,
                    u64::from(self.cluster_count),
                    bits,
                ));
            }
            if t == 0x00 {
                break;
            }
            i += 32;
        }
        None
    }

    /// Read an entire directory (following its cluster chain) into one buffer.
    fn read_directory(&self, first_cluster: u32) -> Option<Vec<u8>> {
        let max_clusters = (MAX_DIR_BYTES / self.cluster_size.max(1)).saturating_add(1);
        let clusters = self.chain(first_cluster, max_clusters.min(1 << 20));
        if clusters.is_empty() {
            return None;
        }
        let mut buf = Vec::new();
        for c in clusters {
            let Some(off) = self.cluster_offset(c) else {
                break;
            };
            if buf.len() as u64 >= MAX_DIR_BYTES {
                break; // DoS guard.
            }
            let chunk = read(&self.src, off, self.cluster_size as usize);
            buf.extend_from_slice(&chunk);
        }
        Some(buf)
    }

    /// Recursively walk a directory, emitting file/dir entries.
    #[allow(clippy::too_many_arguments)]
    fn walk_dir(
        &self,
        first_cluster: u32,
        parent_path: &str,
        depth: u32,
        sink: &mut dyn EntrySink,
        opts: &WalkOpts,
        bitmap: Option<&Bitmap>,
        stats: &mut WalkStats,
        visited: &mut HashSet<u32>,
    ) {
        if depth > 64 || stats.emitted as usize >= opts.max_entries {
            return;
        }
        if !visited.insert(first_cluster) {
            return;
        }
        let Some(dir) = self.read_directory(first_cluster) else {
            return;
        };
        // Subdirectories to descend into once this directory's buffer is dropped.
        let mut subdirs: Vec<(u32, String)> = Vec::new();
        let mut i = 0usize;
        while i + 32 <= dir.len() {
            if stats.emitted as usize >= opts.max_entries {
                return;
            }
            let raw_type = dir.get(i).copied().unwrap_or(0);
            // 0x00 = end of directory (rest is unused).
            if raw_type == 0x00 {
                break;
            }
            let base = raw_type & 0x7F;
            let in_use = raw_type & 0x80 != 0;
            if base == (TYPE_FILE & 0x7F) {
                // File directory entry begins a set.
                let secondary = dir.get(i + 1).copied().unwrap_or(0) as usize;
                let attrs = le_u16(&dir, i + 4);
                let created = exfat_date(le_u32(&dir, i + 8));
                let modified = exfat_date(le_u32(&dir, i + 12));
                let is_dir = attrs & ATTR_DIRECTORY != 0;

                // The Stream entry should immediately follow.
                let stream_off = i + 32;
                let sbase = dir.get(stream_off).map(|t| t & 0x7F).unwrap_or(0);
                if sbase != (TYPE_STREAM & 0x7F) {
                    i += 32;
                    continue;
                }
                let flags = dir.get(stream_off + 1).copied().unwrap_or(0);
                let name_len = dir.get(stream_off + 3).copied().unwrap_or(0) as usize;
                let data_len = le_u64(&dir, stream_off + 24);
                let first = le_u32(&dir, stream_off + 20);
                let no_fat_chain = flags & FLAG_NO_FAT_CHAIN != 0;

                // Name entries follow the Stream entry.
                let name = read_name(&dir, stream_off + 32, secondary.saturating_sub(1), name_len);
                let raw_name = name.as_bytes().to_vec();

                let state = if in_use {
                    EntryState::Live
                } else {
                    EntryState::Deleted
                };
                let include = (in_use && opts.include_live) || (!in_use && opts.include_deleted);

                let child_path = if parent_path.is_empty() {
                    name.clone()
                } else {
                    format!("{parent_path}/{name}")
                };

                if is_dir {
                    if include && !name.is_empty() && name != "." && name != ".." {
                        let id = (first_cluster as u64) << 20 | (i as u64);
                        let mut e = Entry::new_file(id, name.clone(), raw_name, 0, state);
                        e.kind = EntryKind::Dir;
                        e.path = Some(child_path.clone());
                        e.created = created.clone();
                        e.modified = modified.clone();
                        e.confidence = if self.checksum_ok { 0.95 } else { 0.85 };
                        emit(sink, stats, e);
                    }
                    // Recurse into live directories; a deleted subdirectory is
                    // only followed while its first cluster is still free (its
                    // chain is unreliable once reused). Without a bitmap, attempt it.
                    let descend = in_use
                        || bitmap
                            .and_then(|b| {
                                self.cluster_offset(first)
                                    .map(|o| !b.is_offset_allocated(o))
                            })
                            .unwrap_or(true);
                    if first >= 2 && descend {
                        subdirs.push((first, child_path));
                    }
                } else if include {
                    let deleted = !in_use;
                    let (extents, assumed) =
                        self.resolve_extents(first, data_len, no_fat_chain, deleted);
                    let id = (first as u64) << 20 | (i as u64);
                    let mut e = Entry::new_file(id, name.clone(), raw_name, data_len, state);
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
                    let base_conf: f32 = if self.checksum_ok { 0.9 } else { 0.8 };
                    e.confidence = if assumed { base_conf * 0.75 } else { base_conf };
                    emit(sink, stats, e);
                }

                // Advance past the whole set (File + secondary entries).
                i += 32 * (1 + secondary.max(1));
                continue;
            }
            i += 32;
        }
        drop(dir);
        for (first, child_path) in subdirs {
            self.walk_dir(
                first,
                &child_path,
                depth + 1,
                sink,
                opts,
                bitmap,
                stats,
                visited,
            );
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

/// Coalesce an ordered cluster list into byte extents, clamped to `data_len`.
fn coalesce(clusters: &[u32], fs: &ExFat, data_len: u64) -> Vec<Extent> {
    let mut extents: Vec<Extent> = Vec::new();
    let mut remaining = data_len;
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
        let take = remaining.min(fs.cluster_size);
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
        remaining = remaining.saturating_sub(take);
        prev = Some(c);
    }
    if let Some(s) = run_start {
        extents.push(Extent {
            offset: s,
            len: run_len,
        });
    }
    // Trim the final extent so the total equals data_len exactly.
    let total: u64 = extents.iter().map(|e| e.len).sum();
    if total > data_len {
        if let Some(last) = extents.last_mut() {
            last.len = last.len.saturating_sub(total - data_len);
        }
    }
    extents.retain(|e| e.len > 0);
    extents
}

impl FileSystem for ExFat {
    fn kind(&self) -> &'static str {
        "exfat"
    }

    fn block_size(&self) -> u32 {
        u32::try_from(self.cluster_size).unwrap_or(0)
    }

    fn allocation_bitmap(&self) -> Option<Bitmap> {
        self.load_bitmap()
    }

    fn walk(&self, sink: &mut dyn EntrySink, opts: &WalkOpts) -> Result<WalkStats, FsError> {
        // A healthy volume has an up-case table; note its absence as a warning
        // signal (does not block the walk).
        let _upcase = self.has_upcase();
        let bitmap = self.load_bitmap();
        let mut stats = WalkStats::default();
        let mut visited = HashSet::new();
        self.walk_dir(
            self.root_dir_cluster,
            "",
            0,
            sink,
            opts,
            bitmap.as_ref(),
            &mut stats,
            &mut visited,
        );
        let _ = (
            self.fat_length,
            self.number_of_fats,
            self.serial,
            self.bytes_per_sector,
        );
        Ok(stats)
    }
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
fn le_u64(b: &[u8], off: usize) -> u64 {
    match b.get(off..off + 8) {
        Some(s) => {
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            u64::from_le_bytes(a)
        }
        None => 0,
    }
}

/// Reconstruct a UTF-16LE name from the File-Name entries (0xC1/0x41) that
/// follow the Stream entry, truncated to `name_len` UTF-16 code units.
fn read_name(dir: &[u8], mut off: usize, name_entries: usize, name_len: usize) -> String {
    let mut units: Vec<u16> = Vec::new();
    let mut entries = 0usize;
    while entries < name_entries && off + 32 <= dir.len() && units.len() < name_len {
        let base = dir.get(off).map(|t| t & 0x7F).unwrap_or(0);
        if base != (TYPE_NAME & 0x7F) {
            break;
        }
        // 15 UTF-16 chars per Name entry, starting at +2.
        for k in 0..15 {
            if units.len() >= name_len {
                break;
            }
            let u = le_u16(dir, off + 2 + k * 2);
            units.push(u);
        }
        entries += 1;
        off += 32;
    }
    // Strip any trailing NULs that slipped past name_len.
    while units.last() == Some(&0) {
        units.pop();
    }
    String::from_utf16_lossy(&units)
}

/// Read the volume label (entry 0x83) from the root directory.
fn read_volume_label(src: &Arc<dyn BlockSource>, boot: &[u8]) -> Option<String> {
    let bps_shift = boot.get(108).copied().unwrap_or(0);
    let spc_shift = boot.get(109).copied().unwrap_or(0);
    if !(9..=12).contains(&bps_shift) || u32::from(bps_shift) + u32::from(spc_shift) > 25 {
        return None;
    }
    let bytes_per_sector = 1u64 << bps_shift;
    let cluster_size = bytes_per_sector << spc_shift;
    let cluster_heap_offset = (le_u32(boot, 88) as u64).saturating_mul(bytes_per_sector);
    let root = le_u32(boot, 96);
    if root < 2 || cluster_size == 0 {
        return None;
    }
    let off = cluster_heap_offset.saturating_add(u64::from(root - 2).saturating_mul(cluster_size));
    let dir = read(src, off, cluster_size.min(4096) as usize);
    let mut i = 0usize;
    while i + 32 <= dir.len() {
        let t = dir.get(i).copied().unwrap_or(0);
        if t == TYPE_LABEL {
            let n = dir.get(i + 1).copied().unwrap_or(0) as usize;
            let mut units = Vec::new();
            for k in 0..n.min(11) {
                units.push(le_u16(&dir, i + 2 + k * 2));
            }
            return Some(String::from_utf16_lossy(&units));
        }
        if t == 0x00 {
            break;
        }
        i += 32;
    }
    None
}

/// Decode an exFAT timestamp (DOS-style packed dword) to `YYYY-MM-DD`.
fn exfat_date(ts: u32) -> Option<String> {
    if ts == 0 {
        return None;
    }
    let date = (ts >> 16) as u16;
    let day = date & 0x1F;
    let month = (date >> 5) & 0x0F;
    let year = 1980 + ((date >> 9) & 0x7F) as u32;
    if month == 0 || day == 0 || month > 12 || day > 31 {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// Verify the exFAT boot checksum (spec §3.4): a 32-bit rolling checksum over
/// the first 11 sectors of the main boot region, excluding bytes 106, 107 and
/// 112, compared against the repeated value in the checksum sector (sector 11).
fn verify_boot_checksum(src: &Arc<dyn BlockSource>, bytes_per_sector: usize) -> bool {
    if bytes_per_sector == 0 || bytes_per_sector > 4096 {
        return false;
    }
    let region = read(src, 0, bytes_per_sector * 11);
    if region.len() < bytes_per_sector * 11 {
        return false;
    }
    let mut checksum: u32 = 0;
    for (i, &b) in region.iter().enumerate() {
        if i == 106 || i == 107 || i == 112 {
            continue;
        }
        checksum = checksum.rotate_right(1).wrapping_add(u32::from(b));
    }
    let csum_sector = read(src, (bytes_per_sector * 11) as u64, 4);
    le_u32(&csum_sector, 0) == checksum
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn date_decode() {
        // 2026-09-12 → year-1980=46 (<<9), month 9 (<<5), day 12.
        let date: u16 = (46 << 9) | (9 << 5) | 12;
        let ts = (date as u32) << 16;
        assert_eq!(exfat_date(ts).as_deref(), Some("2026-09-12"));
        assert_eq!(exfat_date(0), None);
    }
}
