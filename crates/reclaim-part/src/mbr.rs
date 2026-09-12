//! MBR + EBR extended-partition chain parsing (docs/plan/04 §1).

use crate::{le_u32, mbr_fs_hint, read_bytes, Container, Partition, PartitionMap, Scheme};
use crate::{MBR_SIG_OFF, MBR_TABLE_OFF};
use reclaim_block::BlockSource;
use std::sync::Arc;

/// Extended-partition container types whose first-usable CHS begins an EBR chain.
fn is_extended(t: u8) -> bool {
    matches!(t, 0x05 | 0x0F | 0x85)
}

/// Parse the MBR at `lba0`. Returns `None` if the 0x55AA signature is absent.
/// Extended chains are followed via [`follow_ebr`] by the caller through the
/// returned container entries; we inline that here since we hold `src`.
pub(crate) fn parse(lba0: &[u8], _ss: u64, total: u64) -> Option<PartitionMap> {
    if lba0.get(MBR_SIG_OFF) != Some(&0x55) || lba0.get(MBR_SIG_OFF + 1) != Some(&0xAA) {
        return None;
    }
    let mut entries: Vec<Partition> = Vec::new();
    let mut extended: Vec<(u64, u64)> = Vec::new(); // (ebr_base_lba in bytes, size bytes)
    let mut idx = 1u32;
    for i in 0..4 {
        let base = MBR_TABLE_OFF + i * 16;
        let Some(rec) = lba0.get(base..base + 16) else {
            continue;
        };
        let type_byte = rec.get(4).copied().unwrap_or(0);
        let start_lba = le_u32(rec, 8) as u64;
        let num = le_u32(rec, 12) as u64;
        if type_byte == 0 || num == 0 {
            continue;
        }
        let ss = 512u64; // MBR LBAs are 512-byte units by convention.
        let start = start_lba.saturating_mul(ss);
        let len = num.saturating_mul(ss).min(total.saturating_sub(start));
        if start >= total || len == 0 {
            continue;
        }
        if is_extended(type_byte) {
            extended.push((start, len));
            continue;
        }
        entries.push(Partition {
            index: idx,
            start,
            len,
            scheme: Scheme::Mbr,
            type_byte: Some(type_byte),
            type_guid: None,
            type_label: mbr_type_label(type_byte).to_string(),
            name: None,
            fs_hint: mbr_fs_hint(type_byte),
        });
        idx += 1;
    }

    // We can't follow EBR chains without the source; signal them so the caller
    // can expand. We stash them as notes and let [`expand`] do the reads.
    let mut notes = Vec::new();
    if !extended.is_empty() {
        notes.push(format!(
            "{} extended partition(s) present (EBR chain)",
            extended.len()
        ));
    }

    Some(PartitionMap {
        scheme: Scheme::Mbr,
        entries,
        confidence: 0.9,
        notes,
        containers: Vec::<Container>::new(),
    })
}

/// Follow the EBR chain(s) rooted at the extended partitions in `lba0`, adding
/// logical partitions to `map`. Called by [`crate::scan`] after [`parse`].
pub(crate) fn expand(src: &Arc<dyn BlockSource>, lba0: &[u8], total: u64, map: &mut PartitionMap) {
    let ss = 512u64;
    let mut next_index = map.entries.iter().map(|p| p.index).max().unwrap_or(0) + 1;
    for i in 0..4 {
        let base = MBR_TABLE_OFF + i * 16;
        let Some(rec) = lba0.get(base..base + 16) else {
            continue;
        };
        let type_byte = rec.get(4).copied().unwrap_or(0);
        if !is_extended(type_byte) {
            continue;
        }
        let ext_base = le_u32(rec, 8) as u64 * ss;
        let mut cur = ext_base;
        let mut guard = 0;
        while guard < 256 {
            guard += 1;
            if cur >= total {
                break;
            }
            let ebr = read_bytes(src, cur, 512);
            if ebr.get(MBR_SIG_OFF) != Some(&0x55) || ebr.get(MBR_SIG_OFF + 1) != Some(&0xAA) {
                break;
            }
            // First entry = the logical partition relative to this EBR.
            let Some(e0) = ebr.get(MBR_TABLE_OFF..MBR_TABLE_OFF + 16) else {
                break;
            };
            let t0 = e0.get(4).copied().unwrap_or(0);
            let rel = le_u32(e0, 8) as u64 * ss;
            let num = le_u32(e0, 12) as u64 * ss;
            if t0 != 0 && num != 0 {
                let start = cur.saturating_add(rel);
                let len = num.min(total.saturating_sub(start));
                if start < total && len > 0 {
                    map.entries.push(Partition {
                        index: next_index,
                        start,
                        len,
                        scheme: Scheme::Mbr,
                        type_byte: Some(t0),
                        type_guid: None,
                        type_label: mbr_type_label(t0).to_string(),
                        name: None,
                        fs_hint: mbr_fs_hint(t0),
                    });
                    next_index += 1;
                }
            }
            // Second entry = pointer to the next EBR (relative to the extended base).
            let Some(e1) = ebr.get(MBR_TABLE_OFF + 16..MBR_TABLE_OFF + 32) else {
                break;
            };
            let t1 = e1.get(4).copied().unwrap_or(0);
            let nxt = le_u32(e1, 8) as u64 * ss;
            if t1 == 0 || nxt == 0 {
                break;
            }
            let next_abs = ext_base.saturating_add(nxt);
            if next_abs <= cur {
                break; // non-advancing chain — corrupt; stop.
            }
            cur = next_abs;
        }
    }
}

fn mbr_type_label(t: u8) -> &'static str {
    match t {
        0x01 => "FAT12",
        0x04 | 0x06 => "FAT16",
        0x0B | 0x0C => "FAT32",
        0x07 => "exFAT / NTFS / HPFS",
        0x0E => "FAT16 LBA",
        0x05 | 0x0F | 0x85 => "Extended",
        0x82 => "Linux swap",
        0x83 => "Linux",
        0xAF => "Apple HFS/HFS+",
        0xAB => "Apple Boot",
        0xEE => "GPT protective",
        0xEF => "EFI System",
        _ => "Unknown",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn mbr_with(entries: &[(u8, u32, u32)]) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        for (i, (t, start, num)) in entries.iter().enumerate() {
            let off = MBR_TABLE_OFF + i * 16;
            b[off + 4] = *t;
            b[off + 8..off + 12].copy_from_slice(&start.to_le_bytes());
            b[off + 12..off + 16].copy_from_slice(&num.to_le_bytes());
        }
        b[MBR_SIG_OFF] = 0x55;
        b[MBR_SIG_OFF + 1] = 0xAA;
        b
    }

    #[test]
    fn parses_basic_entries() {
        // The exFAT golden image's MBR: type 0x07 at LBA 63, 524223 sectors.
        let b = mbr_with(&[(0x07, 63, 524223)]);
        let m = parse(&b, 512, 1 << 30).unwrap();
        assert_eq!(m.scheme, Scheme::Mbr);
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].start, 63 * 512);
        assert_eq!(m.entries[0].type_byte, Some(0x07));
        assert_eq!(m.entries[0].fs_hint, Some("exfat/ntfs"));
    }

    #[test]
    fn missing_signature_is_none() {
        let b = vec![0u8; 512];
        assert!(parse(&b, 512, 1 << 20).is_none());
    }

    #[test]
    fn protective_mbr_has_ee_entry() {
        let b = mbr_with(&[(0xEE, 1, 0xFFFF_FFFF)]);
        let m = parse(&b, 512, 1 << 30).unwrap();
        assert_eq!(m.entries[0].type_byte, Some(0xEE));
    }
}
