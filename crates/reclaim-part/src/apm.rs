//! Apple Partition Map detection (detect-only, docs/plan/04 §1).
//!
//! Block 0 is the Driver Descriptor Record (`ER` signature, 0x4552); the
//! partition map entries begin at block 1 with a `PM` signature (0x504D). We
//! report the count for the map; full APM support is a later (T2) concern.

use crate::read_bytes;
use reclaim_block::BlockSource;
use std::sync::Arc;

/// Detect an Apple Partition Map; returns an evidence string on a hit.
pub(crate) fn detect(src: &Arc<dyn BlockSource>, ss: u64) -> Option<String> {
    let b0 = read_bytes(src, 0, 512);
    if b0.get(0..2) != Some(b"ER") {
        return None;
    }
    // First map entry at block 1 must carry the 'PM' signature.
    let b1 = read_bytes(src, ss, 512);
    if b1.get(0..2) != Some(b"PM") {
        return None;
    }
    // map_entry_count is a big-endian u32 at offset 4 of a map entry.
    let count = u32::from_be_bytes([
        b1.get(4).copied().unwrap_or(0),
        b1.get(5).copied().unwrap_or(0),
        b1.get(6).copied().unwrap_or(0),
        b1.get(7).copied().unwrap_or(0),
    ]);
    Some(format!(
        "Apple Partition Map (detect-only): 'ER' at block 0, 'PM' at block 1, {count} entries"
    ))
}
