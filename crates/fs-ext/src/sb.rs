//! ext2/3/4 superblock and block-group descriptors (docs/plan/04 §2 ext row).
//!
//! The superblock lives at byte 1024; block-group descriptor tables follow the
//! superblock block. `sparse_super` places backup superblocks at group 0, 1 and
//! powers of 3/5/7 — used to recover geometry when the primary is zeroed.

use crate::bytes::{le_u16, le_u32};
use reclaim_block::BlockSource;
use std::sync::Arc;

/// ext magic `0xEF53` at superblock offset 0x38.
pub const EXT_MAGIC: u16 = 0xEF53;
const SB_OFF: u64 = 1024;

pub const INCOMPAT_64BIT: u32 = 0x0080;

/// A block group's on-disk descriptor (the fields we use).
#[derive(Copy, Clone, Debug)]
pub struct GroupDesc {
    pub block_bitmap: u64,
    pub inode_table: u64,
}

/// Parsed superblock geometry plus the group descriptor table.
#[derive(Clone, Debug)]
pub struct Superblock {
    pub block_size: u64,
    pub inode_size: u64,
    pub inodes_count: u32,
    pub blocks_count: u64,
    pub first_data_block: u64,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub desc_size: u64,
    pub has_64bit: bool,
    pub journal_inum: u32,
    pub label: Option<String>,
    pub uuid: Option<String>,
    pub groups: Vec<GroupDesc>,
    /// Where the superblock was actually found (0 = primary), for diagnostics.
    pub found_at_group: u32,
}

impl Superblock {
    /// Inode-table byte offset and containing group for inode `ino` (1-based).
    #[must_use]
    pub fn inode_location(&self, ino: u32) -> Option<u64> {
        if ino == 0 || self.inodes_per_group == 0 {
            return None;
        }
        let group = (ino - 1) / self.inodes_per_group;
        let index = u64::from((ino - 1) % self.inodes_per_group);
        let gd = self.groups.get(group as usize)?;
        Some(
            gd.inode_table
                .saturating_mul(self.block_size)
                .saturating_add(index.saturating_mul(self.inode_size)),
        )
    }

    /// Number of block groups.
    #[must_use]
    pub fn group_count(&self) -> u64 {
        if self.blocks_per_group == 0 {
            return 0;
        }
        (self.blocks_count.saturating_sub(self.first_data_block))
            .div_ceil(u64::from(self.blocks_per_group))
    }
}

/// Read and validate a superblock, trying backups if the primary is corrupt.
pub fn read_superblock(src: &Arc<dyn BlockSource>) -> Option<Superblock> {
    if let Some(sb) = parse_at(src, SB_OFF, 0) {
        return Some(sb);
    }
    // Primary is bad: hunt backups. sparse_super groups: 1, then powers of
    // 3/5/7. Backup superblock is the first block of the group.
    for g in backup_groups(4096) {
        // We don't yet know the block size, so try both common ones.
        for bs in [1024u64, 4096, 2048, 8192, 16384, 32768, 65536] {
            let bpg = probe_blocks_per_group(bs);
            let block = u64::from(g) * bpg + first_data_block_for(bs);
            let off = block.saturating_mul(bs);
            if let Some(sb) = parse_at(src, off, g) {
                if sb.block_size == bs {
                    return Some(sb);
                }
            }
        }
    }
    None
}

/// Standard `blocks_per_group` = 8 × block_size (bits in one bitmap block).
fn probe_blocks_per_group(block_size: u64) -> u64 {
    block_size * 8
}

fn first_data_block_for(block_size: u64) -> u64 {
    if block_size == 1024 {
        1
    } else {
        0
    }
}

/// Backup-superblock group numbers within `[1, limit)` (sparse_super layout).
fn backup_groups(limit: u32) -> Vec<u32> {
    let mut out = vec![1u32];
    for base in [3u32, 5, 7] {
        let mut p = base;
        while p < limit {
            out.push(p);
            p = p.saturating_mul(base);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Parse a superblock at byte `off`; `found_group` records where it came from.
fn parse_at(src: &Arc<dyn BlockSource>, off: u64, found_group: u32) -> Option<Superblock> {
    let sb = crate::bytes::read(src, off, 1024);
    if le_u16(&sb, 0x38) != EXT_MAGIC {
        return None;
    }
    let log_bs = le_u32(&sb, 0x18);
    if log_bs > 6 {
        return None; // block size 1KiB..64KiB
    }
    let block_size = 1024u64 << log_bs;
    let inodes_count = le_u32(&sb, 0x00);
    let blocks_lo = u64::from(le_u32(&sb, 0x04));
    let blocks_hi = u64::from(le_u32(&sb, 0x150));
    let first_data_block = u64::from(le_u32(&sb, 0x14));
    let blocks_per_group = le_u32(&sb, 0x20);
    let inodes_per_group = le_u32(&sb, 0x28);
    let rev = le_u32(&sb, 0x4C);
    let inode_size = if rev >= 1 {
        u64::from(le_u16(&sb, 0x58)).max(128)
    } else {
        128
    };
    let incompat = le_u32(&sb, 0x60);
    let has_64bit = incompat & INCOMPAT_64BIT != 0;
    let desc_size = if has_64bit {
        u64::from(le_u16(&sb, 0xFE)).max(64)
    } else {
        32
    };
    let journal_inum = le_u32(&sb, 0xE0);
    if blocks_per_group == 0 || inodes_per_group == 0 || inode_size == 0 {
        return None;
    }
    let blocks_count = if has_64bit {
        (blocks_hi << 32) | blocks_lo
    } else {
        blocks_lo
    };

    let mut me = Superblock {
        block_size,
        inode_size,
        inodes_count,
        blocks_count,
        first_data_block,
        blocks_per_group,
        inodes_per_group,
        desc_size,
        has_64bit,
        journal_inum,
        label: read_str(&sb, 0x78, 16),
        uuid: read_uuid(&sb, 0x68),
        groups: Vec::new(),
        found_at_group: found_group,
    };
    me.groups = read_group_descs(src, &me);
    if me.groups.is_empty() {
        return None;
    }
    Some(me)
}

/// Read the block-group descriptor table (right after the superblock block).
fn read_group_descs(src: &Arc<dyn BlockSource>, sb: &Superblock) -> Vec<GroupDesc> {
    // Bound the group count by what the backing source can actually address, so
    // a corrupt/huge declared block count cannot drive a giant table read.
    let addressable_groups = (src.len() / sb.block_size.max(1))
        .div_ceil(u64::from(sb.blocks_per_group).max(1))
        .saturating_add(1);
    let ngroups = sb.group_count().min(addressable_groups).min(65_536);
    if ngroups == 0 {
        return Vec::new();
    }
    let gdt_block = sb.first_data_block + 1;
    let gdt_off = gdt_block.saturating_mul(sb.block_size);
    let total = ngroups.saturating_mul(sb.desc_size);
    let bytes = crate::bytes::read(src, gdt_off, usize::try_from(total).unwrap_or(0));
    let mut out = Vec::with_capacity(ngroups as usize);
    for g in 0..ngroups {
        let base = usize::try_from(g * sb.desc_size).unwrap_or(usize::MAX);
        let block_bitmap = read_lo_hi(&bytes, base, base + 0x20, sb.has_64bit);
        let inode_table = read_lo_hi(&bytes, base + 8, base + 0x28, sb.has_64bit);
        out.push(GroupDesc {
            block_bitmap,
            inode_table,
        });
    }
    out
}

fn read_lo_hi(b: &[u8], lo: usize, hi: usize, has_64: bool) -> u64 {
    let lo = u64::from(le_u32(b, lo));
    if has_64 {
        lo | (u64::from(le_u32(b, hi)) << 32)
    } else {
        lo
    }
}

fn read_str(b: &[u8], off: usize, len: usize) -> Option<String> {
    let s = b.get(off..off + len)?;
    let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
    let t = String::from_utf8_lossy(s.get(..end)?).into_owned();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn read_uuid(b: &[u8], off: usize) -> Option<String> {
    let u = b.get(off..off + 16)?;
    if u.iter().all(|x| *x == 0) {
        return None;
    }
    Some(
        u.iter()
            .map(|x| format!("{x:02x}"))
            .collect::<Vec<_>>()
            .join(""),
    )
}
