//! APFS space-manager allocation bitmap (Apple File System Reference: "Space
//! Manager"). This yields the container's real free/allocated map, which the
//! recoverability score uses to tell whether a deleted file's blocks have been
//! reused (allocated now ⇒ likely overwritten ⇒ lower score).
//!
//! Chain: the space manager is an *ephemeral* object located through a
//! checkpoint-map in the descriptor ring; its main device points at a
//! chunk-info block (CIB) whose chunk-info entries each reference a per-chunk
//! allocation bitmap block. We assemble those into one packed bitmap.
//!
//! Scope: the common single-CIB case (no chunk-info *address* blocks) is
//! implemented, which covers every container up to ~16 GiB — all of Reclaim's
//! test volumes. A multi-CAB container returns `None` (the score then falls back
//! to the no-bitmap path); this is documented in the Phase-3 build log.

use crate::obj::{le_u32, le_u64, read, ObjPhys};
use reclaim_block::BlockSource;
use reclaim_fs_core::Bitmap;
use std::sync::Arc;

const OBJ_SPACEMAN: u16 = 0x0005;

/// Build the container allocation bitmap given the space-manager physical
/// address (resolved from a checkpoint-map), the block size and total blocks.
#[must_use]
pub fn build(
    src: &Arc<dyn BlockSource>,
    base: u64,
    sm_paddr: u64,
    block_size: u32,
    block_count: u64,
) -> Option<Bitmap> {
    let bs = u64::from(block_size.max(1));
    let sm = read(
        src,
        base.checked_add(sm_paddr.checked_mul(bs)?)?,
        block_size as usize,
    );
    let obj = ObjPhys::parse(&sm)?;
    if obj.kind() != OBJ_SPACEMAN {
        return None;
    }
    // Spec: one bitmap block covers block_size*8 blocks, so blocks_per_chunk
    // must equal exactly that (a crafted value would inflate the bitmap).
    let blocks_per_chunk = le_u32(&sm, 36);
    if blocks_per_chunk == 0 || u64::from(blocks_per_chunk) != bs.saturating_mul(8) {
        return None;
    }
    // Never assemble more than the bitmap for `block_count` blocks (≤ 64 MiB).
    let max_bytes = block_count.div_ceil(8).min(64 << 20) as usize;
    // spaceman_device SD_MAIN begins at +48.
    const DEV: usize = 48;
    let cib_count = le_u32(&sm, DEV + 16);
    let cab_count = le_u32(&sm, DEV + 20);
    let addr_off = le_u32(&sm, DEV + 32) as usize;
    if cab_count != 0 || cib_count == 0 {
        // Multi-CAB layout not handled (see module docs).
        return None;
    }

    let bytes_per_chunk_bitmap = (blocks_per_chunk / 8) as usize;
    let mut bits: Vec<u8> = Vec::new();

    // The CIB address array lives inside the spaceman block at `addr_off`.
    for c in 0..cib_count.min(4096) {
        if bits.len() >= max_bytes {
            break;
        }
        let cib_paddr = le_u64(&sm, addr_off + (c as usize) * 8);
        if cib_paddr == 0 {
            continue;
        }
        let cib = read(
            src,
            base.saturating_add(cib_paddr.saturating_mul(bs)),
            block_size as usize,
        );
        if ObjPhys::parse(&cib).is_none() {
            continue;
        }
        // chunk_info_block: obj(32), cib_index u32(32), cib_chunk_info_count u32(36),
        // then chunk_info[] each 32 bytes — at most (block_size - 40) / 32 fit.
        let n = (le_u32(&cib, 36) as usize).min((block_size as usize).saturating_sub(40) / 32);
        for k in 0..n {
            if bits.len() >= max_bytes {
                break;
            }
            let e = 40 + k * 32;
            // chunk_info: ci_xid u64, ci_addr u64, ci_block_count u32, ci_free_count u32, ci_bitmap_addr u64
            let block_cnt = le_u32(&cib, e + 16);
            let bitmap_addr = le_u64(&cib, e + 24);
            if bitmap_addr == 0 {
                // A zero bitmap address means the whole chunk is free.
                bits.resize(bits.len() + bytes_per_chunk_bitmap, 0);
            } else {
                let bm = read(
                    src,
                    base.saturating_add(bitmap_addr.saturating_mul(bs)),
                    block_size as usize,
                );
                let take = bytes_per_chunk_bitmap.min(bm.len());
                bits.extend_from_slice(bm.get(..take).unwrap_or(&[]));
                if take < bytes_per_chunk_bitmap {
                    bits.resize(bits.len() + (bytes_per_chunk_bitmap - take), 0);
                }
            }
            let _ = block_cnt;
        }
    }

    if bits.is_empty() {
        return None;
    }
    Some(Bitmap::new(block_size, 0, block_count, bits))
}
