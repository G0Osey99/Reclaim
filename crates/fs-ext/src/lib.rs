//! Reclaim ext2/3/4 metadata engine (docs/plan/04 §2 ext row).
//!
//! Implemented from the public ext2/3/4 on-disk layout (build guide Part 1.4
//! rule 2 — primary specs only, no GPL code copied). The engine:
//!
//! * parses the superblock (magic `0xEF53` at byte 1080), with **backup
//!   superblock** fallback (`sparse_super`) when the primary is zeroed,
//! * reads the block-group descriptors (32- or 64-bit) and inode tables,
//! * recovers a deleted file's data from its **inode** — the extent tree
//!   (`0xF30A`, ext4) or the classic direct/indirect block pointers (ext2/3),
//!   which survive a delete on the images we target,
//! * recovers **deleted names** from directory `rec_len` slack (the "absorbed"
//!   entry keeps its inode number + name),
//! * falls back to the **jbd2 journal** for a prior inode copy when the current
//!   one was zeroed, and scans unallocated blocks for surviving `0xF30A` extent
//!   nodes,
//! * assembles the block allocation bitmap for the carver / recoverability score.
//!
//! The engine reads a **volume** source (partition window); extents are
//! volume-relative and shifted to absolute by the session's `EntrySink`.
//!
//! Parses hostile bytes; denies panicking accessors (build guide Part 1.4 rule 3).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

mod bytes;
mod dir;
mod inode;
mod journal;
mod sb;

use bytes::read;
use dir::DirEntry;
use inode::Inode;
use reclaim_block::BlockSource;
use reclaim_fs_core::{
    Bitmap, Entry, EntryKind, EntrySink, EntryState, FileSystem, FsError, Probe, WalkOpts,
    WalkStats,
};
use sb::Superblock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const ROOT_INODE: u32 = 2;
/// Cap on directory inodes visited during the tree walk (DoS guard).
const MAX_DIRS: usize = 2_000_000;
/// Cap on blocks used when assembling the allocation bitmap.
const MAX_BITMAP_BLOCKS: u64 = 64 * 1024 * 1024;

/// An opened ext2/3/4 volume.
pub struct Ext {
    src: Arc<dyn BlockSource>,
    sb: Superblock,
}

impl std::fmt::Debug for Ext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ext")
            .field("block_size", &self.sb.block_size)
            .field("inodes", &self.sb.inodes_count)
            .field("blocks", &self.sb.blocks_count)
            .field("groups", &self.sb.groups.len())
            .field("found_at_group", &self.sb.found_at_group)
            .finish()
    }
}

/// Probe a volume source for ext2/3/4 (docs/plan/04 §2).
#[must_use]
pub fn probe(src: &Arc<dyn BlockSource>) -> Option<Probe> {
    let s = sb::read_superblock(src)?;
    // A backup-only find is lower confidence (primary was corrupt).
    let confidence = if s.found_at_group == 0 { 0.95 } else { 0.8 };
    Some(Probe {
        fs_kind: "ext",
        confidence,
        block_size: u32::try_from(s.block_size).unwrap_or(0),
        label: s.label.clone(),
        uuid: s.uuid.clone(),
        total_bytes: s.blocks_count.saturating_mul(s.block_size),
    })
}

/// Open an ext volume (consumes the [`Probe`]).
pub fn open(src: Arc<dyn BlockSource>, _probe: Probe) -> Result<Ext, FsError> {
    let sb = sb::read_superblock(&src)
        .ok_or_else(|| FsError::NotThisFs("no ext superblock (primary or backup)".into()))?;
    if sb.groups.is_empty() {
        return Err(FsError::Corrupt("ext: no group descriptors".into()));
    }
    Ok(Ext { src, sb })
}

/// Boxed [`open`] for the engine registry.
pub fn open_boxed(src: Arc<dyn BlockSource>, probe: Probe) -> Result<Box<dyn FileSystem>, FsError> {
    Ok(Box::new(open(src, probe)?))
}

impl FileSystem for Ext {
    fn kind(&self) -> &'static str {
        "ext"
    }

    fn block_size(&self) -> u32 {
        u32::try_from(self.sb.block_size).unwrap_or(4096)
    }

    fn allocation_bitmap(&self) -> Option<Bitmap> {
        self.build_bitmap()
    }

    fn walk(&self, sink: &mut dyn EntrySink, opts: &WalkOpts) -> Result<WalkStats, FsError> {
        let mut ctx = WalkCtx {
            engine: self,
            sink,
            opts,
            stats: WalkStats::default(),
            visited_dirs: HashSet::new(),
            emitted_inodes: HashSet::new(),
            journal: HashMap::new(),
        };
        // Journal-recovered prior inode copies (used when a current inode's map
        // was zeroed). Best-effort; empty on ext2 / no journal.
        ctx.journal = journal::recover_from_journal(&self.src, &self.sb);
        ctx.walk_tree()?;
        ctx.orphan_inode_scan()?;
        ctx.extent_node_scan()?;
        Ok(ctx.stats)
    }
}

impl Ext {
    /// Assemble the per-group block bitmaps into one [`Bitmap`] indexed by block.
    fn build_bitmap(&self) -> Option<Bitmap> {
        let sb = &self.sb;
        // Clamp to the source-addressable block count: blocks past EOF are
        // out-of-range (treated allocated) so there is no need to represent them.
        let addressable = self.src.len() / sb.block_size.max(1) + 1;
        let block_count = sb.blocks_count.min(addressable);
        if block_count == 0 || block_count > MAX_BITMAP_BLOCKS {
            return None;
        }
        let nbytes = usize::try_from(block_count.div_ceil(8)).ok()?;
        let mut bits = vec![0xFFu8; nbytes]; // default allocated (conservative)
        for (g, gd) in sb.groups.iter().enumerate() {
            let bm = read(
                &self.src,
                gd.block_bitmap.saturating_mul(sb.block_size),
                sb.block_size as usize,
            );
            let group_first = sb
                .first_data_block
                .saturating_add((g as u64).saturating_mul(u64::from(sb.blocks_per_group)));
            for i in 0..u64::from(sb.blocks_per_group) {
                let block = group_first.saturating_add(i);
                if block >= block_count {
                    break;
                }
                let src_byte = (i / 8) as usize;
                let src_bit = (i % 8) as u8;
                let set = bm
                    .get(src_byte)
                    .map(|b| (b >> src_bit) & 1 == 1)
                    .unwrap_or(true);
                let dst_byte = (block / 8) as usize;
                let dst_bit = (block % 8) as u8;
                if let Some(bref) = bits.get_mut(dst_byte) {
                    if set {
                        *bref |= 1 << dst_bit;
                    } else {
                        *bref &= !(1 << dst_bit);
                    }
                }
            }
        }
        Some(Bitmap::new(
            u32::try_from(sb.block_size).unwrap_or(4096),
            0,
            block_count,
            bits,
        ))
    }
}

/// State carried through the recursive walk.
struct WalkCtx<'a> {
    engine: &'a Ext,
    sink: &'a mut dyn EntrySink,
    opts: &'a WalkOpts,
    stats: WalkStats,
    visited_dirs: HashSet<u32>,
    emitted_inodes: HashSet<u32>,
    journal: HashMap<u32, Inode>,
}

impl WalkCtx<'_> {
    fn budget_left(&self) -> bool {
        (self.stats.emitted as usize) < self.opts.max_entries
    }

    /// BFS the directory tree from the root inode.
    fn walk_tree(&mut self) -> Result<(), FsError> {
        // (dir_inode, path). Root's path is "".
        let mut queue: Vec<(u32, String)> = vec![(ROOT_INODE, String::new())];
        while let Some((dino, path)) = queue.pop() {
            if !self.budget_left() || self.visited_dirs.len() > MAX_DIRS {
                break;
            }
            if !self.visited_dirs.insert(dino) {
                continue;
            }
            let Some(dir_inode) = Inode::read(&self.engine.src, &self.engine.sb, dino) else {
                continue;
            };
            if !dir_inode.is_dir() {
                continue;
            }
            let entries = dir::read_dir(&self.engine.src, &self.engine.sb, &dir_inode);
            for e in entries {
                if e.name == "." || e.name == ".." || e.name == "lost+found" {
                    continue;
                }
                let child_path = if path.is_empty() {
                    e.name.clone()
                } else {
                    format!("{path}/{}", e.name)
                };
                if !e.deleted {
                    // Live entry.
                    if is_dir_entry(&e) {
                        queue.push((e.inode, child_path));
                    } else {
                        self.emit_from_inode(&e, dino, &child_path, EntryState::Live);
                    }
                } else {
                    // Deleted entry recovered from slack.
                    self.emit_from_inode(&e, dino, &child_path, EntryState::Deleted);
                }
                if !self.budget_left() {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Emit an entry for directory record `e`, resolving extents from its inode
    /// (or a journal copy if the current inode's map was zeroed).
    fn emit_from_inode(&mut self, e: &DirEntry, parent: u32, path: &str, state: EntryState) {
        if !self.budget_left() {
            return;
        }
        let sb = &self.engine.sb;
        let src = &self.engine.src;
        let mut inode = Inode::read(src, sb, e.inode);
        // If the live inode's map is empty but the journal has a copy, use it.
        if state != EntryState::Live {
            let empty = inode
                .as_ref()
                .map(|i| i.i_block.iter().all(|b| *b == 0))
                .unwrap_or(true);
            if empty {
                if let Some(j) = self.journal.get(&e.inode) {
                    inode = Some(j.clone());
                }
            }
        }
        let Some(inode) = inode else { return };
        // Skip directories as file results (they are traversed, not recovered).
        if inode.is_dir() {
            return;
        }
        let extents = inode.extents(src, sb);
        let size = if inode.size > 0 {
            inode.size
        } else {
            extents.iter().fold(0u64, |a, x| a.saturating_add(x.len))
        };
        let mut entry = Entry::new_file(
            u64::from(e.inode),
            e.name.clone(),
            e.raw_name.clone(),
            size,
            state,
        );
        entry.parent_id = Some(u64::from(parent));
        entry.kind = if inode.is_regular() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        entry.path = Some(path.to_string());
        entry.created = date_from_epoch(inode.ctime);
        entry.modified = date_from_epoch(inode.mtime);
        entry.extents = extents;
        entry.confidence = match state {
            EntryState::Live => 1.0,
            _ => {
                // Higher when the inode still looks deleted (metadata intact).
                if inode.looks_deleted() {
                    0.85
                } else {
                    0.6
                }
            }
        };
        self.record(entry);
    }

    /// Scan every inode for freed-but-intact regular files not already emitted.
    fn orphan_inode_scan(&mut self) -> Result<(), FsError> {
        let sb = &self.engine.sb;
        let src = &self.engine.src;
        if !self.opts.include_deleted {
            return Ok(());
        }
        // First inode usable for user data is s_first_ino (rev1 = 11). Bound the
        // scan by the inodes the group table can actually address (a corrupt
        // superblock may declare billions).
        let first = 11u32;
        let addressable = (sb.groups.len() as u64).saturating_mul(u64::from(sb.inodes_per_group));
        let count = u64::from(sb.inodes_count).min(addressable).min(8_000_000);
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        for ino in first..=count {
            if !self.budget_left() {
                break;
            }
            if self.emitted_inodes.contains(&ino) || self.visited_dirs.contains(&ino) {
                continue;
            }
            let Some(inode) = Inode::read(src, sb, ino) else {
                continue;
            };
            if !inode.is_regular() || !inode.looks_deleted() || inode.size == 0 {
                continue;
            }
            let extents = inode.extents(src, sb);
            if extents.is_empty() {
                continue;
            }
            let name = format!("inode_{ino}");
            let mut entry = Entry::new_file(
                u64::from(ino),
                name.clone(),
                name.into_bytes(),
                inode.size,
                EntryState::Orphaned,
            );
            entry.path = None;
            entry.created = date_from_epoch(inode.ctime);
            entry.modified = date_from_epoch(inode.mtime);
            entry.extents = extents;
            entry.confidence = 0.6;
            self.record(entry);
        }
        Ok(())
    }

    /// Recover deleted files whose inode is gone, via surviving `0xF30A` extent
    /// nodes in unallocated blocks (docs/plan/04 §2).
    fn extent_node_scan(&mut self) -> Result<(), FsError> {
        if !self.opts.include_deleted || !self.budget_left() {
            return Ok(());
        }
        let bitmap = self.engine.build_bitmap();
        let allocated = |b: u64| -> bool {
            bitmap
                .as_ref()
                .map(|bm| bm.is_block_allocated(b))
                .unwrap_or(true)
        };
        let runs = journal::scan_extent_nodes(&self.engine.src, &self.engine.sb, &allocated);
        let mut idx = 0u64;
        for exts in runs {
            if !self.budget_left() {
                break;
            }
            idx += 1;
            let total: u64 = exts.iter().fold(0u64, |a, x| a.saturating_add(x.len));
            let name = format!("extentnode_{idx}");
            let mut entry = Entry::new_file(
                0xF30A_0000_0000_0000u64 | idx,
                name.clone(),
                name.into_bytes(),
                total,
                EntryState::Orphaned,
            );
            entry.path = None;
            entry.extents = exts;
            entry.confidence = 0.4;
            self.record(entry);
        }
        Ok(())
    }

    /// Emit an entry, updating stats + the emitted-inode set.
    fn record(&mut self, entry: Entry) {
        let ino = entry.id as u32;
        if entry.state.is_recoverable_target() {
            self.stats.deleted += 1;
        } else {
            self.stats.live += 1;
        }
        self.stats.emitted += 1;
        self.emitted_inodes.insert(ino);
        self.sink.emit(entry);
    }
}

fn is_dir_entry(e: &DirEntry) -> bool {
    // file_type 2 = directory (filetype feature); if unknown (0), fall back to
    // treating it as a file (the inode read will correct dir vs file).
    e.file_type == 2
}

/// Convert a Unix epoch (seconds) to `YYYY-MM-DD` (UTC), best effort.
fn date_from_epoch(secs: u32) -> Option<String> {
    if secs == 0 {
        return None;
    }
    let days = i64::from(secs) / 86_400;
    let (y, m, d) = civil_from_days(days);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// Howard Hinnant's days→civil algorithm (public domain).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn date_conv() {
        assert_eq!(date_from_epoch(0), None);
        // 2026-09-12 ~ epoch 1_789_000_000-ish; just check it parses to a date.
        let d = date_from_epoch(1_789_000_000).unwrap();
        assert_eq!(&d[..2], "20");
    }
}
