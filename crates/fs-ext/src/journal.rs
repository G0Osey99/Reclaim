//! ext3/4 recovery aids for inodes whose block map was zeroed on delete
//! (docs/plan/04 §2 ext row): the jbd2 journal often holds a *prior* copy of an
//! inode-table block, and surviving `0xF30A` extent-tree nodes in unallocated
//! blocks pin a deleted file's data runs even when its inode is gone.

use crate::bytes::{le_u16, le_u32, read};
use crate::inode::Inode;
use crate::sb::Superblock;
use reclaim_block::BlockSource;
use reclaim_fs_core::Extent;
use std::collections::HashMap;
use std::sync::Arc;

const JBD2_MAGIC: u32 = 0xC03B_3998; // big-endian on disk
const EXTENT_MAGIC: u16 = 0xF30A;
/// Bound on journal blocks scanned.
const MAX_JOURNAL_BLOCKS: u64 = 262_144; // ≤ 256 K blocks
/// Bound on unallocated blocks scanned for extent nodes.
const MAX_SCAN_BLOCKS: u64 = 1024 * 1024;
/// Bound on inodes harvested from journal copies.
const MAX_JOURNAL_INODES: usize = 1_000_000;

/// jbd2 journal superblock `s_feature_incompat` bits.
const JBD2_INCOMPAT_64BIT: u32 = 0x2;
const JBD2_INCOMPAT_CSUM_V2: u32 = 0x8;
const JBD2_INCOMPAT_CSUM_V3: u32 = 0x10;

/// jbd2 tag flags.
const JBD2_FLAG_SAME_UUID: u32 = 0x2;
const JBD2_FLAG_LAST_TAG: u32 = 0x8;

/// True if fs block `b` lies in any group's inode table.
fn in_inode_table(sb: &Superblock, b: u64) -> bool {
    let blocks_per_group_it =
        u64::from(sb.inodes_per_group).saturating_mul(sb.inode_size) / sb.block_size.max(1);
    sb.groups
        .iter()
        .any(|g| b >= g.inode_table && b < g.inode_table.saturating_add(blocks_per_group_it.max(1)))
}

/// Scan the jbd2 journal for prior copies of inode-table blocks and return, for
/// each inode number, a recovered [`Inode`] that still carries a data map.
///
/// Best-effort: unknown tag variants or checksums are ignored (we only read).
pub fn recover_from_journal(src: &Arc<dyn BlockSource>, sb: &Superblock) -> HashMap<u32, Inode> {
    let mut out: HashMap<u32, Inode> = HashMap::new();
    if sb.journal_inum == 0 || sb.journal_inum > sb.inodes_count {
        return out;
    }
    let Some(jinode) = Inode::read(src, sb, sb.journal_inum) else {
        return out;
    };
    // The journal file's data blocks, in logical order.
    let jexts = jinode.extents(src, sb);
    let bs = sb.block_size;
    // Read the journal superblock (first journal block) for the tag size hint.
    let jblocks: Vec<u64> = {
        let mut v = Vec::new();
        for e in &jexts {
            let mut o = e.offset;
            while o < e.offset.saturating_add(e.len) {
                v.push(o / bs.max(1));
                o = o.saturating_add(bs);
                if v.len() as u64 > MAX_JOURNAL_BLOCKS {
                    break;
                }
            }
        }
        v
    };
    if jblocks.is_empty() {
        return out;
    }
    // The journal superblock (first journal block) decides the tag layout.
    let jsb = read(
        src,
        jblocks.first().copied().unwrap_or(0).saturating_mul(bs),
        0x2C,
    );
    let j_incompat = if be_u32(&jsb, 0) == JBD2_MAGIC {
        be_u32(&jsb, 0x28)
    } else {
        0
    };
    // Walk journal blocks: a descriptor block (type 1) is followed by data
    // blocks, one per tag, each a copy of the fs block named in the tag.
    let mut i = 0usize;
    while i < jblocks.len() {
        let jb = jblocks.get(i).copied().unwrap_or(0);
        let hdr = read(src, jb.saturating_mul(bs), 12);
        if be_u32(&hdr, 0) != JBD2_MAGIC {
            i += 1;
            continue;
        }
        let btype = be_u32(&hdr, 4);
        if btype != 1 {
            i += 1;
            continue; // only descriptor blocks carry tags
        }
        // Parse tags to learn which fs block each following data block maps to.
        let desc = read(src, jb.saturating_mul(bs), bs as usize);
        let targets = parse_descriptor_tags(j_incompat, &desc);
        let mut data_idx = i + 1;
        for target in targets {
            let Some(jbn) = jblocks.get(data_idx).copied() else {
                break;
            };
            data_idx += 1;
            if in_inode_table(sb, target) {
                let blk = read(src, jbn.saturating_mul(bs), bs as usize);
                harvest_inode_block(sb, target, &blk, &mut out);
                if out.len() >= MAX_JOURNAL_INODES {
                    return out;
                }
            }
        }
        i = data_idx.max(i + 1);
    }
    out
}

/// Parse a jbd2 descriptor block's tags → the fs block numbers they map.
///
/// Tag layout depends on the journal's incompat features:
/// * CSUM_V3: 16 bytes — blocknr u32, flags u32, blocknr_high u32, checksum u32.
/// * otherwise: blocknr u32, checksum u16, flags u16, [blocknr_high u32 if
///   64BIT], plus 2 trailing checksum bytes if CSUM_V2.
///
/// All fields are big-endian; the tag stream starts after the 12-byte header.
fn parse_descriptor_tags(j_incompat: u32, desc: &[u8]) -> Vec<u64> {
    let csum_v3 = j_incompat & JBD2_INCOMPAT_CSUM_V3 != 0;
    let has_64 = j_incompat & JBD2_INCOMPAT_64BIT != 0;
    let tag_size = if csum_v3 {
        16
    } else {
        let mut n = if has_64 { 12 } else { 8 };
        if j_incompat & JBD2_INCOMPAT_CSUM_V2 != 0 {
            n += 2;
        }
        n
    };
    let mut out = Vec::new();
    let mut p = 12usize;
    let mut guard = 0;
    while p + tag_size <= desc.len() {
        guard += 1;
        if guard > 100_000 {
            break;
        }
        let blocknr_lo = u64::from(be_u32(desc, p));
        let (flags, blocknr_hi) = if csum_v3 {
            (be_u32(desc, p + 4), u64::from(be_u32(desc, p + 8)))
        } else {
            let hi = if has_64 {
                u64::from(be_u32(desc, p + 8))
            } else {
                0
            };
            (u32::from(be_u16(desc, p + 6)), hi)
        };
        let blocknr = if csum_v3 || has_64 {
            (blocknr_hi << 32) | blocknr_lo
        } else {
            blocknr_lo
        };
        out.push(blocknr);
        // Without SAME_UUID a 16-byte UUID follows the tag.
        let mut adv = tag_size;
        if flags & JBD2_FLAG_SAME_UUID == 0 {
            adv += 16;
        }
        p += adv;
        if flags & JBD2_FLAG_LAST_TAG != 0 {
            break;
        }
        if out.len() > 4096 {
            break;
        }
    }
    out
}

/// Extract inodes from a recovered inode-table block copy, preferring ones with
/// a non-empty data map (the point of consulting the journal).
fn harvest_inode_block(sb: &Superblock, fs_block: u64, blk: &[u8], out: &mut HashMap<u32, Inode>) {
    // Which group/table offset this block sits at → first inode number here.
    let blocks_per_group_it =
        u64::from(sb.inodes_per_group).saturating_mul(sb.inode_size) / sb.block_size.max(1);
    for (gi, g) in sb.groups.iter().enumerate() {
        if fs_block >= g.inode_table
            && fs_block < g.inode_table.saturating_add(blocks_per_group_it.max(1))
        {
            let block_in_table = fs_block - g.inode_table;
            let inodes_per_block = sb.block_size / sb.inode_size.max(1);
            let first_ino_index = block_in_table.saturating_mul(inodes_per_block);
            for k in 0..inodes_per_block {
                let idx = first_ino_index.saturating_add(k);
                let ino = (gi as u64)
                    .saturating_mul(u64::from(sb.inodes_per_group))
                    .saturating_add(idx)
                    .saturating_add(1);
                let Ok(ino32) = u32::try_from(ino) else {
                    continue;
                };
                let base = usize::try_from(k.saturating_mul(sb.inode_size)).unwrap_or(usize::MAX);
                let Some(raw) = blk.get(base..base + sb.inode_size as usize) else {
                    continue;
                };
                if let Some(inode) = parse_inode_bytes(raw) {
                    if inode.size > 0 && !inode_map_empty(&inode) {
                        out.entry(ino32).or_insert(inode);
                    }
                }
            }
            return;
        }
    }
}

fn inode_map_empty(inode: &Inode) -> bool {
    inode.i_block.iter().all(|b| *b == 0)
}

/// Parse an inode from raw bytes (journal copy has no table offset).
fn parse_inode_bytes(raw: &[u8]) -> Option<Inode> {
    if raw.len() < 128 {
        return None;
    }
    let mut i_block = [0u8; 60];
    i_block.copy_from_slice(raw.get(0x28..0x28 + 60)?);
    let mode = le_u16(raw, 0x00);
    let size_lo = u64::from(le_u32(raw, 0x04));
    let size_hi = u64::from(le_u32(raw, 0x6C));
    let size = if mode & 0xF000 == 0x8000 {
        (size_hi << 32) | size_lo
    } else {
        size_lo
    };
    Some(Inode {
        mode,
        size,
        dtime: le_u32(raw, 0x14),
        links: le_u16(raw, 0x1A),
        flags: le_u32(raw, 0x20),
        ctime: le_u32(raw, 0x0C),
        mtime: le_u32(raw, 0x10),
        i_block,
    })
}

/// Scan unallocated blocks for `0xF30A` extent-tree leaf nodes and return the
/// byte-extent lists they describe — deleted files whose inode is gone but whose
/// extent nodes survive. Emitted as unnamed orphan candidates.
pub fn scan_extent_nodes(
    src: &Arc<dyn BlockSource>,
    sb: &Superblock,
    allocated: &dyn Fn(u64) -> bool,
) -> Vec<Vec<Extent>> {
    let mut found = Vec::new();
    let bs = sb.block_size;
    // Bound by the source-addressable block count and a hard cap.
    let addressable = src.len() / bs.max(1) + 1;
    let total = sb.blocks_count.min(addressable).min(MAX_SCAN_BLOCKS);
    for b in sb.first_data_block..total {
        if allocated(b) {
            continue;
        }
        let node = read(src, b.saturating_mul(bs), core::cmp::min(bs, 4096) as usize);
        if le_u16(&node, 0) != EXTENT_MAGIC {
            continue;
        }
        let depth = le_u16(&node, 6);
        let entries = le_u16(&node, 2);
        if depth != 0 || entries == 0 || entries > 340 {
            continue; // only self-contained leaf nodes
        }
        let mut exts = Vec::new();
        let mut total_bytes = 0u64;
        for i in 0..entries as usize {
            let base = 12 + i * 12;
            if node.get(base..base + 12).is_none() {
                break;
            }
            let mut len = le_u16(&node, base + 4);
            if len == 0 || len > 32768 {
                if len > 32768 {
                    len -= 32768;
                } else {
                    continue;
                }
            }
            let start_hi = u64::from(le_u16(&node, base + 6));
            let start_lo = u64::from(le_u32(&node, base + 8));
            let phys = (start_hi << 32) | start_lo;
            if phys == 0 || phys >= sb.blocks_count {
                continue;
            }
            let off = phys.saturating_mul(bs);
            let bytes = u64::from(len).saturating_mul(bs);
            total_bytes = total_bytes.saturating_add(bytes);
            exts.push(Extent {
                offset: off,
                len: bytes,
            });
        }
        if !exts.is_empty() && total_bytes > 0 {
            found.push(exts);
        }
        if found.len() > 100_000 {
            break;
        }
    }
    found
}

#[inline]
fn be_u16(b: &[u8], off: usize) -> u16 {
    b.get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_be_bytes)
        .unwrap_or(0)
}

#[inline]
fn be_u32(b: &[u8], off: usize) -> u32 {
    b.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_be_bytes)
        .unwrap_or(0)
}
