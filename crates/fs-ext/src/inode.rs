//! ext inode parsing and data-extent recovery (docs/plan/04 §2 ext row).
//!
//! Two block-mapping schemes are handled:
//! * **Extent tree** (ext4, `EXTENTS_FL`): an `0xF30A` header in `i_block`, with
//!   interior index nodes resolved through data blocks (bounded depth).
//! * **Classic block map** (ext2/3): 12 direct + single/double/triple indirect
//!   `u32` pointers.
//!
//! Both survive a debugfs/orphan delete on the images we target, so a deleted
//! file's inode still yields its extents; the jbd2 journal and the `0xF30A`
//! unallocated scan (see `journal`) cover inodes whose map was zeroed.

use crate::bytes::{le_u16, le_u32, read};
use crate::sb::Superblock;
use reclaim_block::BlockSource;
use reclaim_fs_core::Extent;
use std::sync::Arc;

const EXTENT_MAGIC: u16 = 0xF30A;
const EXTENTS_FL: u32 = 0x0008_0000;
const S_IFMT: u16 = 0xF000;
const S_IFREG: u16 = 0x8000;
const S_IFDIR: u16 = 0x4000;

/// Cap on data blocks collected for one file (DoS guard on crafted maps).
const MAX_FILE_BLOCKS: usize = 8 * 1024 * 1024;
/// Cap on extent-tree interior depth.
const MAX_EXTENT_DEPTH: u16 = 5;

/// A parsed inode (only the fields the engine uses).
#[derive(Clone, Debug)]
pub struct Inode {
    pub mode: u16,
    pub size: u64,
    pub dtime: u32,
    pub links: u16,
    pub flags: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub i_block: [u8; 60],
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }
    pub fn is_regular(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }
    /// A freed inode still bearing a mode/size — a delete left its metadata.
    pub fn looks_deleted(&self) -> bool {
        self.dtime != 0 || self.links == 0
    }

    /// Read inode `ino` (1-based) from the inode table.
    pub fn read(src: &Arc<dyn BlockSource>, sb: &Superblock, ino: u32) -> Option<Inode> {
        let off = sb.inode_location(ino)?;
        let raw = read(src, off, sb.inode_size as usize);
        if raw.len() < 128 {
            return None;
        }
        let mut i_block = [0u8; 60];
        i_block.copy_from_slice(raw.get(0x28..0x28 + 60)?);
        let size_lo = u64::from(le_u32(&raw, 0x04));
        let size_hi = u64::from(le_u32(&raw, 0x6C)); // regular files only
        let mode = le_u16(&raw, 0x00);
        let size = if mode & S_IFMT == S_IFREG {
            (size_hi << 32) | size_lo
        } else {
            size_lo
        };
        Some(Inode {
            mode,
            size,
            dtime: le_u32(&raw, 0x14),
            links: le_u16(&raw, 0x1A),
            flags: le_u32(&raw, 0x20),
            ctime: le_u32(&raw, 0x0C),
            mtime: le_u32(&raw, 0x10),
            i_block,
        })
    }

    /// The file's byte extents (volume-relative), trimmed to the logical size.
    pub fn extents(&self, src: &Arc<dyn BlockSource>, sb: &Superblock) -> Vec<Extent> {
        let mut blocks: Vec<(u64, u64)> = Vec::new(); // (logical_block, physical_block)
        if self.flags & EXTENTS_FL != 0 {
            collect_extent_tree(src, sb, &self.i_block, 0, &mut blocks);
        } else {
            collect_classic(src, sb, &self.i_block, &mut blocks);
        }
        runs_to_extents(sb, &mut blocks, self.size)
    }
}

/// Parse an extent header + entries from a 12+ byte buffer, recursing into
/// interior nodes through data blocks.
fn collect_extent_tree(
    src: &Arc<dyn BlockSource>,
    sb: &Superblock,
    node: &[u8],
    depth_guard: u16,
    out: &mut Vec<(u64, u64)>,
) {
    if depth_guard > MAX_EXTENT_DEPTH || out.len() > MAX_FILE_BLOCKS {
        return;
    }
    if le_u16(node, 0) != EXTENT_MAGIC {
        return;
    }
    let entries = le_u16(node, 2);
    let depth = le_u16(node, 6);
    let max = le_u16(node, 4);
    if entries > max.max(entries) {
        return;
    }
    for i in 0..entries as usize {
        let base = 12 + i * 12;
        if node.get(base..base + 12).is_none() {
            break;
        }
        if depth == 0 {
            // Leaf extent.
            let logical = u64::from(le_u32(node, base));
            let mut len = le_u16(node, base + 4);
            if len > 32768 {
                len -= 32768; // uninitialized extent
            }
            let start_hi = u64::from(le_u16(node, base + 6));
            let start_lo = u64::from(le_u32(node, base + 8));
            let phys = (start_hi << 32) | start_lo;
            for b in 0..u64::from(len) {
                if out.len() > MAX_FILE_BLOCKS {
                    return;
                }
                out.push((logical.saturating_add(b), phys.saturating_add(b)));
            }
        } else {
            // Index node → child block.
            let leaf_lo = u64::from(le_u32(node, base + 4));
            let leaf_hi = u64::from(le_u16(node, base + 8));
            let child = (leaf_hi << 32) | leaf_lo;
            let block = read(
                src,
                child.saturating_mul(sb.block_size),
                sb.block_size as usize,
            );
            collect_extent_tree(src, sb, &block, depth_guard + 1, out);
        }
    }
}

/// Classic 12 direct + single/double/triple indirect block pointers.
fn collect_classic(
    src: &Arc<dyn BlockSource>,
    sb: &Superblock,
    i_block: &[u8],
    out: &mut Vec<(u64, u64)>,
) {
    let mut logical = 0u64;
    // 12 direct.
    for i in 0..12 {
        let b = u64::from(le_u32(i_block, i * 4));
        push_data_block(&mut logical, b, out);
    }
    let ptrs_per_block = sb.block_size / 4;
    // Single indirect.
    let si = u64::from(le_u32(i_block, 48));
    walk_indirect(src, sb, si, 1, ptrs_per_block, &mut logical, out);
    // Double indirect.
    let di = u64::from(le_u32(i_block, 52));
    walk_indirect(src, sb, di, 2, ptrs_per_block, &mut logical, out);
    // Triple indirect.
    let ti = u64::from(le_u32(i_block, 56));
    walk_indirect(src, sb, ti, 3, ptrs_per_block, &mut logical, out);
}

fn walk_indirect(
    src: &Arc<dyn BlockSource>,
    sb: &Superblock,
    block: u64,
    level: u8,
    ppb: u64,
    logical: &mut u64,
    out: &mut Vec<(u64, u64)>,
) {
    if block == 0 || out.len() > MAX_FILE_BLOCKS {
        return;
    }
    let data = read(
        src,
        block.saturating_mul(sb.block_size),
        sb.block_size as usize,
    );
    for i in 0..ppb {
        let idx = usize::try_from(i * 4).unwrap_or(usize::MAX);
        let b = u64::from(le_u32(&data, idx));
        if level == 1 {
            push_data_block(logical, b, out);
        } else {
            walk_indirect(src, sb, b, level - 1, ppb, logical, out);
        }
        if out.len() > MAX_FILE_BLOCKS {
            return;
        }
    }
}

fn push_data_block(logical: &mut u64, phys: u64, out: &mut Vec<(u64, u64)>) {
    if phys != 0 {
        out.push((*logical, phys));
    }
    *logical += 1;
}

/// Coalesce (logical, physical) block pairs into byte extents, trimming to size.
fn runs_to_extents(sb: &Superblock, blocks: &mut [(u64, u64)], size: u64) -> Vec<Extent> {
    blocks.sort_by_key(|(l, _)| *l);
    let bs = sb.block_size;
    let mut extents: Vec<Extent> = Vec::new();
    for &(_lb, pb) in blocks.iter() {
        let off = pb.saturating_mul(bs);
        if let Some(last) = extents.last_mut() {
            if last.offset.saturating_add(last.len) == off {
                last.len = last.len.saturating_add(bs);
                continue;
            }
        }
        extents.push(Extent {
            offset: off,
            len: bs,
        });
    }
    // Trim total to the logical size.
    trim_to_size(&mut extents, size);
    extents
}

/// Trim an extent list so the total byte length is exactly `size`.
pub fn trim_to_size(extents: &mut Vec<Extent>, size: u64) {
    if size == 0 {
        extents.clear();
        return;
    }
    let mut acc = 0u64;
    let mut keep = 0usize;
    for (i, e) in extents.iter_mut().enumerate() {
        if acc >= size {
            keep = i;
            break;
        }
        let remain = size - acc;
        if e.len > remain {
            e.len = remain;
        }
        acc = acc.saturating_add(e.len);
        keep = i + 1;
    }
    extents.truncate(keep);
}
