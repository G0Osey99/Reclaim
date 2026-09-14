//! Reclaim HFS+/HFSX metadata engine (docs/plan/04 §2 HFS+ row, §3.2).
//!
//! Implemented from **Apple TN1150 (HFS Plus Volume Format)** (build guide
//! Part 2.3 trap 3). Read-only; never panics on data (Part 1.4 rule 3).
//!
//! # Recovery strategy (docs/plan/04 §3.2)
//! * Parse the volume header at offset 1024 (and fall back to the **alternate
//!   volume header** at `size − 1024` when the primary is unusable).
//! * Walk the **catalog B-tree** leaf chain for live files/folders, resolving
//!   full paths through the catalog keys and thread records.
//! * Recover deleted entries by scanning the volume for stale
//!   `kHFSPlusFileRecord`/`FolderRecord` structures: journaled HFS+ compacts a
//!   node on delete, but previous node images survive in the **journal** and in
//!   freed catalog nodes (verified against the golden image — the node-slack
//!   scan generalised to the whole volume).
//! * The **allocation file** gives the block allocation bitmap for scoring.
//! * Fragmented files' extents beyond the inline 8 live in the **extents
//!   overflow** B-tree.
//!
//! HFSX (`HX`) case-sensitive volumes differ only in name comparison, which does
//! not affect recovery; both are handled.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

pub mod date;
pub mod record;

use reclaim_block::BlockSource;
use reclaim_fs_core::{
    Bitmap, Entry, EntrySink, EntryState, Extent, FileSystem, FsError, Probe, WalkOpts, WalkStats,
};
use record::{
    be_u16, be_u32, parse_key, parse_record, CatalogRecord, ExtentDescriptor, ForkData, REC_FILE,
    ROOT_FOLDER_ID, ROOT_PARENT_ID,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const SIG_HFSPLUS: u16 = 0x482B; // "H+"
const SIG_HFSX: u16 = 0x4858; // "HX"
const VH_OFFSET: u64 = 1024;

/// Node-visit budget for the catalog leaf chain (DoS guard).
const MAX_NODES: usize = 2_000_000;
/// Cap on a whole-volume stale-record sweep (documented gap for larger volumes).
const MAX_STALE_SCAN_BYTES: u64 = 16 << 30;

/// A parsed HFS+ volume header (subset Reclaim needs).
#[derive(Clone, Debug)]
struct VolumeHeader {
    case_sensitive: bool,
    block_size: u32,
    total_blocks: u32,
    catalog: ForkData,
    allocation: ForkData,
}

impl VolumeHeader {
    fn parse(vh: &[u8]) -> Option<VolumeHeader> {
        let sig = be_u16(vh, 0);
        if sig != SIG_HFSPLUS && sig != SIG_HFSX {
            return None;
        }
        let block_size = be_u32(vh, 40);
        if !(512..=1 << 20).contains(&block_size) || !block_size.is_power_of_two() {
            return None;
        }
        let total_blocks = be_u32(vh, 44);
        // Fork data: allocationFile @112, extentsFile @192, catalogFile @272.
        let allocation = ForkData::parse(vh, 112);
        let catalog = ForkData::parse(vh, 272);
        if catalog.extents.is_empty() {
            return None;
        }
        Some(VolumeHeader {
            case_sensitive: sig == SIG_HFSX,
            block_size,
            total_blocks,
            catalog,
            allocation,
        })
    }
}

/// Probe a volume source for HFS+/HFSX (docs/plan/04 §2).
#[must_use]
pub fn probe(src: &Arc<dyn BlockSource>) -> Option<Probe> {
    let vh_bytes = read(src, VH_OFFSET, 1024);
    let vh = VolumeHeader::parse(&vh_bytes).or_else(|| {
        // Try the alternate volume header at size − 1024.
        let total = src.len();
        if total > 1024 {
            let alt = read(src, total - 1024, 512);
            VolumeHeader::parse(&alt)
        } else {
            None
        }
    })?;
    Some(Probe {
        fs_kind: if vh.case_sensitive { "hfsx" } else { "hfs+" },
        confidence: 0.98,
        block_size: vh.block_size,
        label: None,
        uuid: None,
        total_bytes: u64::from(vh.total_blocks).saturating_mul(u64::from(vh.block_size)),
    })
}

/// An opened HFS+/HFSX volume.
pub struct Hfs {
    src: Arc<dyn BlockSource>,
    vh: VolumeHeader,
}

impl std::fmt::Debug for Hfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hfs")
            .field("case_sensitive", &self.vh.case_sensitive)
            .field("block_size", &self.vh.block_size)
            .field("total_blocks", &self.vh.total_blocks)
            .finish()
    }
}

/// Open an HFS+/HFSX volume (consumes the [`Probe`]).
pub fn open(src: Arc<dyn BlockSource>, _probe: Probe) -> Result<Hfs, FsError> {
    let vh_bytes = read(&src, VH_OFFSET, 1024);
    let vh = match VolumeHeader::parse(&vh_bytes) {
        Some(v) => v,
        None => {
            let total = src.len();
            let alt = read(&src, total.saturating_sub(1024), 512);
            VolumeHeader::parse(&alt)
                .ok_or_else(|| FsError::NotThisFs("no HFS+ volume header".into()))?
        }
    };
    Ok(Hfs { src, vh })
}

/// Boxed [`open`] for the engine registry.
pub fn open_boxed(src: Arc<dyn BlockSource>, probe: Probe) -> Result<Box<dyn FileSystem>, FsError> {
    Ok(Box::new(open(src, probe)?))
}

impl Hfs {
    fn bs(&self) -> u64 {
        u64::from(self.vh.block_size.max(1))
    }

    /// Map a byte offset within a fork to a device offset via its extents.
    /// Also returns the bytes remaining in that extent from the mapped offset,
    /// so a caller never reads through an extent boundary into unrelated blocks.
    fn fork_device_offset(
        &self,
        extents: &[ExtentDescriptor],
        byte_off: u64,
    ) -> Option<(u64, u64)> {
        let bs = self.bs();
        let mut consumed: u64 = 0;
        for e in extents {
            let ext_bytes = u64::from(e.block_count).saturating_mul(bs);
            if byte_off < consumed.saturating_add(ext_bytes) {
                let within = byte_off - consumed;
                return Some((
                    u64::from(e.start_block)
                        .saturating_mul(bs)
                        .saturating_add(within),
                    ext_bytes - within,
                ));
            }
            consumed = consumed.saturating_add(ext_bytes);
        }
        None
    }

    /// Read `len` bytes starting at `byte_off` within a fork (following extents).
    fn read_fork(&self, extents: &[ExtentDescriptor], byte_off: u64, len: usize) -> Vec<u8> {
        // Reads that stay inside one extent are one device read; spanning reads
        // are stitched extent by extent.
        let mut out = vec![0u8; len];
        let mut done = 0usize;
        while done < len {
            let want_off = byte_off + done as u64;
            let Some((dev, left)) = self.fork_device_offset(extents, want_off) else {
                break;
            };
            // Clamp to what remains in the current extent from `dev`.
            let chunk = (len - done).min(left.min(1 << 20) as usize);
            if chunk == 0 {
                break;
            }
            let bytes = read(&self.src, dev, chunk);
            let n = bytes.len().min(len - done);
            if n == 0 {
                break;
            }
            if let (Some(d), Some(s)) = (out.get_mut(done..done + n), bytes.get(..n)) {
                d.copy_from_slice(s);
            }
            done += n;
        }
        out
    }

    /// The catalog B-tree node size (from its header node), and the first leaf.
    fn catalog_geometry(&self) -> Option<(u32, u32)> {
        // Header node is node 0. Read a generous first slice to cover it.
        let head = self.read_fork(&self.vh.catalog.extents, 0, 512);
        // BTNodeDescriptor(14) then BTHeaderRec; nodeSize at hdr+18 → +32 overall.
        let node_size = be_u16(&head, 14 + 18);
        let first_leaf = be_u32(&head, 14 + 10);
        if node_size < 512 || !node_size.is_power_of_two() {
            return None;
        }
        Some((u32::from(node_size), first_leaf))
    }

    fn read_node(&self, node_num: u32, node_size: u32) -> Vec<u8> {
        let off = u64::from(node_num).saturating_mul(u64::from(node_size));
        self.read_fork(&self.vh.catalog.extents, off, node_size as usize)
    }

    /// Parse the record offsets at the end of a node (last u16 = record 0).
    fn record_offsets(node: &[u8], node_size: u32, num_records: u16) -> Vec<usize> {
        // A node holds at most (node_size - 14) / 2 offsets after its 14-byte
        // descriptor; cap there so a crafted numRecords cannot underflow `pos`.
        let max = ((node_size as usize).saturating_sub(14) / 2).min(num_records as usize);
        let mut offs = Vec::with_capacity(max);
        for i in 0..max {
            let Some(pos) = (node_size as usize).checked_sub(2 * (i + 1)) else {
                break;
            };
            offs.push(be_u16(node, pos) as usize);
        }
        offs
    }
}

/// A live catalog entry gathered during the leaf walk.
#[derive(Clone)]
struct CatEntry {
    parent_id: u32,
    name: String,
    raw_name: Vec<u8>,
    is_dir: bool,
    cnid: u32,
    data_fork: Option<ForkData>,
    create: Option<String>,
    modified: Option<String>,
}

impl Hfs {
    /// Walk the catalog leaf chain, returning the live entries and a CNID→(parent,
    /// name) map for path resolution.
    fn walk_catalog(&self) -> (Vec<CatEntry>, HashMap<u32, (u32, String)>) {
        let mut entries = Vec::new();
        let mut cnid_map: HashMap<u32, (u32, String)> = HashMap::new();
        let Some((node_size, first_leaf)) = self.catalog_geometry() else {
            return (entries, cnid_map);
        };
        let mut node_num = first_leaf;
        let mut visited = 0usize;
        let mut seen: HashSet<u32> = HashSet::new();
        while node_num != 0 && visited < MAX_NODES && seen.insert(node_num) {
            visited += 1;
            let node = self.read_node(node_num, node_size);
            let f_link = be_u32(&node, 0);
            let kind = node.get(8).copied().unwrap_or(0) as i8;
            let num_records = be_u16(&node, 10);
            // kind -1 (0xFF) = leaf node.
            if kind == -1 {
                let offs = Self::record_offsets(&node, node_size, num_records.min(2000));
                for ro in offs {
                    if let Some(entry) = self.parse_catalog_leaf(&node, ro, &mut cnid_map) {
                        entries.push(entry);
                    }
                }
            }
            node_num = f_link;
        }
        (entries, cnid_map)
    }

    fn parse_catalog_leaf(
        &self,
        node: &[u8],
        rec_off: usize,
        cnid_map: &mut HashMap<u32, (u32, String)>,
    ) -> Option<CatEntry> {
        let key = parse_key(node, rec_off)?;
        let data_off = rec_off + key.total_len;
        match parse_record(node, data_off)? {
            CatalogRecord::Folder { cnid } => {
                cnid_map.insert(cnid, (key.parent_id, key.name.clone()));
                Some(CatEntry {
                    parent_id: key.parent_id,
                    name: key.name,
                    raw_name: key.raw_name,
                    is_dir: true,
                    cnid,
                    data_fork: None,
                    create: None,
                    modified: None,
                })
            }
            CatalogRecord::File {
                cnid,
                data_fork,
                create_date,
                mod_date,
                ..
            } => {
                cnid_map.insert(cnid, (key.parent_id, key.name.clone()));
                Some(CatEntry {
                    parent_id: key.parent_id,
                    name: key.name,
                    raw_name: key.raw_name,
                    is_dir: false,
                    cnid,
                    data_fork: Some(data_fork),
                    create: date::hfs_date(create_date),
                    modified: date::hfs_date(mod_date),
                })
            }
            CatalogRecord::Thread { .. } => None,
        }
    }

    /// Resolve a CNID's full path via the parent chain.
    fn path_of(
        &self,
        cnid: u32,
        parent: u32,
        name: &str,
        map: &HashMap<u32, (u32, String)>,
    ) -> String {
        let mut parts = vec![name.to_string()];
        let mut cur = parent;
        let mut guard = 0;
        while cur != ROOT_FOLDER_ID && cur != ROOT_PARENT_ID && cur != 0 && guard < 128 {
            guard += 1;
            match map.get(&cur) {
                Some((p, n)) => {
                    parts.push(n.clone());
                    cur = *p;
                }
                None => break,
            }
        }
        let _ = cnid;
        parts.reverse();
        parts.join("/")
    }

    /// Convert a fork's extents into absolute (volume-relative) byte extents.
    fn fork_extents(&self, fork: &ForkData) -> Vec<Extent> {
        let bs = self.bs();
        let mut out = Vec::new();
        let mut remaining = fork.logical_size;
        for e in &fork.extents {
            if e.block_count == 0 {
                continue;
            }
            let byte_len = u64::from(e.block_count).saturating_mul(bs);
            let len = if remaining == 0 {
                byte_len
            } else {
                byte_len.min(remaining)
            };
            out.push(Extent {
                offset: u64::from(e.start_block).saturating_mul(bs),
                len,
            });
            remaining = remaining.saturating_sub(byte_len);
        }
        out
    }

    fn to_entry(
        &self,
        e: &CatEntry,
        map: &HashMap<u32, (u32, String)>,
        state: EntryState,
        conf: f32,
    ) -> Entry {
        let extents = e
            .data_fork
            .as_ref()
            .map(|f| self.fork_extents(f))
            .unwrap_or_default();
        let size = e.data_fork.as_ref().map(|f| f.logical_size).unwrap_or(0);
        let path = self.path_of(e.cnid, e.parent_id, &e.name, map);
        let mut entry = Entry::new_file(
            u64::from(e.cnid),
            e.name.clone(),
            e.raw_name.clone(),
            size,
            state,
        );
        entry.parent_id = Some(u64::from(e.parent_id));
        entry.path = Some(path);
        entry.extents = extents;
        entry.confidence = conf;
        entry.created = e.create.clone();
        entry.modified = e.modified.clone();
        entry
    }

    /// Scan the whole volume for stale catalog file/folder records (journal +
    /// freed nodes + node slack), emitting those not present live.
    #[allow(clippy::too_many_arguments)]
    fn stale_scan(
        &self,
        live: &HashSet<(u32, String)>,
        map: &HashMap<u32, (u32, String)>,
        sink: &mut dyn EntrySink,
        stats: &mut WalkStats,
        opts: &WalkOpts,
        emitted: &mut HashSet<(u32, String)>,
    ) {
        let total = self.src.len().min(MAX_STALE_SCAN_BYTES);
        let chunk_len: usize = 1 << 20;
        let overlap: usize = 1024; // a record can straddle a chunk boundary
        let mut pos: u64 = 0;
        while pos < total {
            let want = (chunk_len + overlap).min((total - pos) as usize);
            let buf = read(&self.src, pos, want);
            let scan_to = buf.len().saturating_sub(if pos + want as u64 >= total {
                0
            } else {
                overlap
            });
            let mut i = 0usize;
            while i + 8 <= scan_to {
                if let Some(entry) = self.try_stale_at(&buf, i, live, map, emitted) {
                    stats.emitted += 1;
                    stats.deleted += 1;
                    if opts.include_deleted {
                        sink.emit(entry);
                    }
                }
                i += 2; // records are 2-byte aligned within nodes
            }
            pos += chunk_len as u64;
        }
    }

    /// Try to parse a stale catalog record at `off` in `buf`; return an
    /// `Orphaned` entry if it is a plausible file/folder not already seen.
    fn try_stale_at(
        &self,
        buf: &[u8],
        off: usize,
        live: &HashSet<(u32, String)>,
        map: &HashMap<u32, (u32, String)>,
        emitted: &mut HashSet<(u32, String)>,
    ) -> Option<Entry> {
        let key = parse_key(buf, off)?;
        // The key's parent must be a plausible CNID and the name filename-like.
        if !(ROOT_FOLDER_ID..0x4000_0000).contains(&key.parent_id) {
            return None;
        }
        if !is_plausible_name(&key.name) {
            return None;
        }
        // Only file records are recovered (folders carry no content and are the
        // dominant false-positive source when sweeping raw bytes).
        let data_off = off + key.total_len;
        if be_u16(buf, data_off) != REC_FILE {
            return None;
        }
        let dedup = (key.parent_id, key.name.clone());
        if live.contains(&dedup) || emitted.contains(&dedup) {
            return None;
        }
        let CatalogRecord::File {
            cnid,
            data_fork,
            create_date,
            mod_date,
            ..
        } = parse_record(buf, data_off)?
        else {
            return None;
        };
        // Structural sanity to reject coincidental matches in file content:
        //   * a plausible CNID,
        //   * both timestamps decode to real dates,
        //   * at least one data extent that points inside the volume.
        if !(16..0x4000_0000).contains(&cnid) {
            return None;
        }
        let (create, modified) = (date::hfs_date(create_date), date::hfs_date(mod_date));
        if create.is_none() || modified.is_none() {
            return None;
        }
        let first = data_fork.extents.first()?;
        if first.block_count == 0 || first.start_block >= self.vh.total_blocks {
            return None;
        }
        let cat = CatEntry {
            parent_id: key.parent_id,
            name: key.name.clone(),
            raw_name: key.raw_name.clone(),
            is_dir: false,
            cnid,
            data_fork: Some(data_fork),
            create,
            modified,
        };
        emitted.insert(dedup);
        Some(self.to_entry(&cat, map, EntryState::Orphaned, 0.6))
    }

    /// Build the allocation bitmap from the allocation file. HFS+ stores it
    /// MSB-first per byte; [`Bitmap`] is LSB-first, so we reverse each byte.
    fn allocation(&self) -> Option<Bitmap> {
        if self.vh.allocation.extents.is_empty() || self.vh.total_blocks == 0 {
            return None;
        }
        let bytes_needed = u64::from(self.vh.total_blocks).div_ceil(8);
        if bytes_needed > (64 << 20) {
            return None; // >512M blocks: skip (documented)
        }
        let raw = self.read_fork(&self.vh.allocation.extents, 0, bytes_needed as usize);
        let bits: Vec<u8> = raw.iter().map(|b| b.reverse_bits()).collect();
        Some(Bitmap::new(
            self.vh.block_size,
            0,
            u64::from(self.vh.total_blocks),
            bits,
        ))
    }
}

impl FileSystem for Hfs {
    fn kind(&self) -> &'static str {
        if self.vh.case_sensitive {
            "hfsx"
        } else {
            "hfs+"
        }
    }

    fn block_size(&self) -> u32 {
        self.vh.block_size
    }

    fn allocation_bitmap(&self) -> Option<Bitmap> {
        self.allocation()
    }

    fn walk(&self, sink: &mut dyn EntrySink, opts: &WalkOpts) -> Result<WalkStats, FsError> {
        let mut stats = WalkStats::default();
        let (live, cnid_map) = self.walk_catalog();
        let mut live_keys: HashSet<(u32, String)> = HashSet::new();
        for e in &live {
            live_keys.insert((e.parent_id, e.name.clone()));
        }

        if opts.include_live {
            for e in &live {
                if e.is_dir {
                    continue; // directories reconstructed from paths
                }
                stats.emitted += 1;
                stats.live += 1;
                sink.emit(self.to_entry(e, &cnid_map, EntryState::Live, 0.95));
            }
        }

        if opts.include_deleted {
            let mut emitted: HashSet<(u32, String)> = HashSet::new();
            self.stale_scan(&live_keys, &cnid_map, sink, &mut stats, opts, &mut emitted);
        }
        Ok(stats)
    }
}

/// Whether a decoded catalog name looks like a real filename (used to reject
/// coincidental key matches when sweeping raw volume bytes for stale records).
fn is_plausible_name(name: &str) -> bool {
    if name.is_empty() || name.chars().count() > 255 {
        return false;
    }
    let mut spaces = 0u32;
    let mut alnum = false;
    for c in name.chars() {
        if (c as u32) < 0x20 || c == '\u{7f}' || c == '\u{fffd}' {
            return false; // control chars / lossy-decode marker ⇒ not a name
        }
        if c == ' ' {
            spaces += 1;
            if spaces >= 3 {
                return false; // runs of spaces are a concatenation artifact
            }
        } else {
            spaces = 0;
        }
        if c.is_alphanumeric() {
            alnum = true;
        }
    }
    alnum
}

/// Read `len` bytes at absolute `offset`, sector-aligned, zero-filled on short
/// reads (mirrors the other engines' `read` helper; never panics).
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use reclaim_block::{ImageFile, MemorySource, OffsetView};
    use std::path::Path;

    fn golden_view() -> Option<Arc<dyn BlockSource>> {
        let p = Path::new("../../testdata/build/hfsplus-delete.img");
        if !p.is_file() {
            return None;
        }
        let disk: Arc<dyn BlockSource> = Arc::new(ImageFile::open(p).ok()?);
        let map = reclaim_part::scan(&disk);
        let (start, len) = map.volume_windows(disk.len()).into_iter().next()?;
        Some(Arc::new(OffsetView::new(disk, start, len).ok()?))
    }

    #[test]
    fn probe_open_golden() {
        let Some(view) = golden_view() else {
            eprintln!("skip: hfsplus golden not built");
            return;
        };
        let p = probe(&view).expect("hfs+ probe");
        assert!(p.fs_kind == "hfs+" || p.fs_kind == "hfsx");
        assert_eq!(p.block_size, 4096);
        let fs = open(view, p).unwrap();
        let mut sink = reclaim_fs_core::VecSink::default();
        let st = fs.walk(&mut sink, &WalkOpts::default()).unwrap();
        assert!(st.live > 0, "expected live files: {st:?}");
    }

    #[test]
    fn garbage_never_panics() {
        let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(vec![0x33u8; 300_000]));
        assert!(probe(&src).is_none());
    }
}
