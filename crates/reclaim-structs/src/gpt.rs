//! GPT primary + **backup** header recovery (docs/plan/04 §1). When the primary
//! partition table is wiped, the backup GPT at the last LBA still lists every
//! partition — each becomes a [`ProposedVolume`]. Known Apple/Microsoft/Linux
//! type GUIDs give a filesystem hint; the session confirms with an engine probe.

use crate::{le_u32, le_u64, read, ProposedVolume};
use reclaim_block::BlockSource;
use std::sync::Arc;

const SIG: &[u8; 8] = b"EFI PART";
const MAX_ENTRIES: u32 = 4096;

/// Scan for primary (LBA 1) and backup (last LBA) GPT headers and emit a
/// proposal per real partition entry.
pub(crate) fn scan_gpt(src: &Arc<dyn BlockSource>, out: &mut Vec<ProposedVolume>) {
    let lba = u64::from(src.sector_size().max(512));
    // Primary header at LBA 1.
    parse_header(src, lba, false, out);
    // Backup header at the last LBA.
    let total = src.len();
    if total >= lba {
        parse_header(src, total - lba, true, out);
    }
}

fn parse_header(
    src: &Arc<dyn BlockSource>,
    hdr_off: u64,
    backup: bool,
    out: &mut Vec<ProposedVolume>,
) {
    let hdr = read(src, hdr_off, 96);
    if hdr.get(0..8) != Some(SIG) {
        return;
    }
    let lba = u64::from(src.sector_size().max(512));
    let entries_lba = le_u64(&hdr, 72);
    let num = le_u32(&hdr, 80).min(MAX_ENTRIES);
    let esize = le_u32(&hdr, 84);
    if esize < 128 || num == 0 {
        return;
    }
    let table_off = entries_lba.saturating_mul(lba);
    let table = read(
        src,
        table_off,
        (num as usize)
            .saturating_mul(esize as usize)
            .min(4 * 1024 * 1024),
    );
    let src_label = if backup { "backup GPT" } else { "GPT" };
    for i in 0..num as usize {
        let base = i.saturating_mul(esize as usize);
        let Some(ent) = table.get(base..base + 128) else {
            break;
        };
        // Zero type GUID ⇒ unused entry.
        if ent.get(0..16).is_some_and(|g| g.iter().all(|b| *b == 0)) {
            continue;
        }
        let first = le_u64(ent, 32);
        let last = le_u64(ent, 40);
        if first == 0 || last < first {
            continue;
        }
        let start = first.saturating_mul(lba);
        let len = (last - first + 1).saturating_mul(lba);
        let (fs, hint) = fs_from_guid(ent.get(0..16).unwrap_or(&[]));
        let name = utf16_name(ent.get(56..128).unwrap_or(&[]));
        let evidence = format!(
            "{src_label} entry {} [{hint}]{} @ LBA {first}..{last}",
            i + 1,
            name.map(|n| format!(" \"{n}\"")).unwrap_or_default()
        );
        out.push(ProposedVolume {
            start,
            len,
            fs: fs.to_string(),
            confidence: if backup { 0.85 } else { 0.9 },
            evidence,
        });
    }
}

/// Map a GPT partition type GUID (on-disk bytes) to an fs hint.
fn fs_from_guid(guid: &[u8]) -> (&'static str, &'static str) {
    const APFS: [u8; 16] = [
        0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC,
        0xAC,
    ];
    const HFS: [u8; 16] = [
        0x00, 0x53, 0x46, 0x48, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC,
        0xAC,
    ];
    const MSBASIC: [u8; 16] = [
        0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99,
        0xC7,
    ];
    const LINUX: [u8; 16] = [
        0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D,
        0xE4,
    ];
    const EFISYS: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ];
    match guid {
        g if g == APFS => ("apfs", "Apple APFS"),
        g if g == HFS => ("hfsplus", "Apple HFS+"),
        g if g == MSBASIC => ("windows", "Microsoft basic data"),
        g if g == LINUX => ("ext", "Linux filesystem"),
        g if g == EFISYS => ("fat", "EFI system"),
        _ => ("unknown", "unknown type"),
    }
}

/// Decode a UTF-16LE partition name field, trimming trailing NULs.
fn utf16_name(raw: &[u8]) -> Option<String> {
    let mut units = Vec::new();
    let mut i = 0;
    while i + 1 < raw.len() {
        let u = crate::le_u16(raw, i);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    if units.is_empty() {
        None
    } else {
        Some(String::from_utf16_lossy(&units))
    }
}
