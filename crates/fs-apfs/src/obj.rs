//! Low-level APFS object primitives: byte readers, the `obj_phys_t` header, and
//! the Fletcher-64 checksum (Apple File System Reference, "Objects").
//!
//! Every on-disk read goes through the bounds-checked helpers here so the parser
//! never panics on hostile bytes (build guide Part 1.4 rule 3).

use reclaim_block::BlockSource;
use std::sync::Arc;

/// `nx_obj_phys_t` header size (Apple File System Reference: `obj_phys_t`).
pub const OBJ_PHYS_SIZE: usize = 32;

// ---- object type / subtype constants (o_type low 16 bits) ----

/// `OBJECT_TYPE_NX_SUPERBLOCK` (container superblock).
pub const OBJ_NX_SUPERBLOCK: u16 = 0x0001;
/// `OBJECT_TYPE_BTREE` (a B-tree root node).
pub const OBJ_BTREE: u16 = 0x0002;
/// `OBJECT_TYPE_BTREE_NODE` (a non-root B-tree node).
pub const OBJ_BTREE_NODE: u16 = 0x0003;
/// `OBJECT_TYPE_OMAP` (an object-map).
pub const OBJ_OMAP: u16 = 0x000b;
/// `OBJECT_TYPE_CHECKPOINT_MAP`.
pub const OBJ_CHECKPOINT_MAP: u16 = 0x000c;
/// `OBJECT_TYPE_FS` (a volume superblock, `apfs_superblock_t`).
pub const OBJ_FS: u16 = 0x000d;

/// B-tree subtype `OBJECT_TYPE_FSTREE` (a filesystem records tree).
pub const SUB_FSTREE: u16 = 0x000e;
/// B-tree subtype `OBJECT_TYPE_OMAP` (an object-map tree).
pub const SUB_OMAP: u16 = 0x000b;

/// A parsed `obj_phys_t` header (the first 32 bytes of every managed object).
#[derive(Copy, Clone, Debug)]
pub struct ObjPhys {
    /// Stored Fletcher-64 checksum (`o_cksum`).
    pub cksum: u64,
    /// Object identifier (`o_oid`).
    pub oid: u64,
    /// Transaction identifier (`o_xid`).
    pub xid: u64,
    /// Object type field (`o_type`, includes storage/flag bits in the top half).
    pub o_type: u32,
    /// Object subtype (`o_subtype`).
    pub subtype: u32,
}

impl ObjPhys {
    /// Parse the 32-byte header from the front of `block`.
    #[must_use]
    pub fn parse(block: &[u8]) -> Option<ObjPhys> {
        if block.len() < OBJ_PHYS_SIZE {
            return None;
        }
        Some(ObjPhys {
            cksum: le_u64(block, 0),
            oid: le_u64(block, 8),
            xid: le_u64(block, 16),
            o_type: le_u32(block, 24),
            subtype: le_u32(block, 28),
        })
    }

    /// The object storage/type value (`o_type` low 16 bits).
    #[must_use]
    pub fn kind(&self) -> u16 {
        (self.o_type & 0xffff) as u16
    }

    /// The B-tree subtype (`o_subtype` low 16 bits).
    #[must_use]
    pub fn sub_kind(&self) -> u16 {
        (self.subtype & 0xffff) as u16
    }
}

/// Verify the Fletcher-64 checksum stored in the first 8 bytes of `block`
/// against the rest of the block (Apple File System Reference: the checksum is a
/// modified Fletcher over the object body, folded so the stored 8 bytes make the
/// whole block sum to a known value).
///
/// Algorithm (matching Apple's `fletcher64`): two 32-bit running sums modulo
/// 2^32−1 over the little-endian 32-bit words *after* the 8-byte checksum field.
#[must_use]
pub fn fletcher64_valid(block: &[u8]) -> bool {
    if block.len() < 8 {
        return false;
    }
    let stored = le_u64(block, 0);
    stored == fletcher64_check(block)
}

/// Compute the expected value of the 8-byte checksum field for `block`.
#[must_use]
pub fn fletcher64_check(block: &[u8]) -> u64 {
    const MOD: u64 = 0xffff_ffff;
    let body = block.get(8..).unwrap_or(&[]);
    let mut sum1: u64 = 0;
    let mut sum2: u64 = 0;
    let words = body.len() / 4;
    for i in 0..words {
        let w = u64::from(le_u32(body, i * 4));
        sum1 = (sum1 + w) % MOD;
        sum2 = (sum2 + sum1) % MOD;
    }
    let c1 = MOD - ((sum1 + sum2) % MOD);
    let c2 = MOD - ((sum1 + c1) % MOD);
    (c2 << 32) | c1
}

// ---- bounds-checked little-endian readers (never panic) ----

/// Read a little-endian `u16` at `off`, or 0 if out of range.
#[must_use]
pub fn le_u16(b: &[u8], off: usize) -> u16 {
    match b.get(off..off + 2) {
        Some(s) => u16::from_le_bytes([
            s.first().copied().unwrap_or(0),
            s.get(1).copied().unwrap_or(0),
        ]),
        None => 0,
    }
}

/// Read a little-endian `u32` at `off`, or 0 if out of range.
#[must_use]
pub fn le_u32(b: &[u8], off: usize) -> u32 {
    match b.get(off..off + 4) {
        Some(s) => {
            let mut a = [0u8; 4];
            a.copy_from_slice(s);
            u32::from_le_bytes(a)
        }
        None => 0,
    }
}

/// Read a little-endian `u64` at `off`, or 0 if out of range.
#[must_use]
pub fn le_u64(b: &[u8], off: usize) -> u64 {
    match b.get(off..off + 8) {
        Some(s) => {
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            u64::from_le_bytes(a)
        }
        None => 0,
    }
}

/// Read a little-endian `i64` at `off`, or 0 if out of range.
#[must_use]
pub fn le_i64(b: &[u8], off: usize) -> i64 {
    le_u64(b, off) as i64
}

/// Read `len` bytes at absolute `offset` from `src`, sector-aligning the
/// underlying read and zero-filling anything unavailable (mirrors the other
/// engines' `read` helper — corruption/short reads are values, not panics).
#[must_use]
pub fn read(src: &Arc<dyn BlockSource>, offset: u64, len: usize) -> Vec<u8> {
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
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn fletcher_roundtrip() {
        // Build a block, compute its checksum, place it, and verify.
        let mut blk = vec![0u8; 4096];
        for (i, b) in blk.iter_mut().enumerate().skip(8) {
            *b = (i * 7) as u8;
        }
        let ck = fletcher64_check(&blk);
        blk[0..8].copy_from_slice(&ck.to_le_bytes());
        assert!(fletcher64_valid(&blk));
        // Corrupt one byte → invalid.
        blk[100] ^= 0xff;
        assert!(!fletcher64_valid(&blk));
    }

    #[test]
    fn readers_bounds() {
        let b = [1u8, 0, 0, 0];
        assert_eq!(le_u32(&b, 0), 1);
        assert_eq!(le_u32(&b, 2), 0); // out of range → 0
        assert_eq!(le_u64(&b, 0), 0); // not enough bytes → 0
    }
}
