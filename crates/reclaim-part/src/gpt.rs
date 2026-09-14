//! GPT parsing: primary + backup header, CRC-32 validation, Apple type GUIDs
//! (docs/plan/04 §1).

use crate::crc::crc32;
use crate::guid::{guid_to_string, is_zero_guid, type_label};
use crate::{le_u32, le_u64, read_bytes, Container, Partition, PartitionMap, Scheme};
use reclaim_block::BlockSource;
use std::sync::Arc;

const GPT_SIG: &[u8; 8] = b"EFI PART";

struct Header {
    entry_lba: u64,
    num_entries: u32,
    entry_size: u32,
    entries_crc: u32,
    header_crc_ok: bool,
}

fn parse_header(buf: &[u8]) -> Option<Header> {
    if buf.get(0..8) != Some(GPT_SIG.as_slice()) {
        return None;
    }
    let header_size = le_u32(buf, 12) as usize;
    if !(92..=512).contains(&header_size) {
        return None;
    }
    let stored_crc = le_u32(buf, 16);
    // CRC is computed over header_size bytes with the CRC field zeroed.
    let mut hdr = buf.get(0..header_size)?.to_vec();
    if let Some(slot) = hdr.get_mut(16..20) {
        slot.fill(0);
    }
    let header_crc_ok = crc32(&hdr) == stored_crc;
    Some(Header {
        entry_lba: le_u64(buf, 72),
        num_entries: le_u32(buf, 80),
        entry_size: le_u32(buf, 84),
        entries_crc: le_u32(buf, 88),
        header_crc_ok,
    })
}

/// Parse GPT from `src`. Tries the primary header at LBA 1; if its CRC fails,
/// falls back to the backup header at the last LBA. Returns `None` if neither
/// header has the `EFI PART` signature.
pub(crate) fn parse(src: &Arc<dyn BlockSource>, ss: u64, total: u64) -> Option<PartitionMap> {
    let mut notes: Vec<String> = Vec::new();

    let primary_buf = read_bytes(src, ss, 512);
    let primary = parse_header(&primary_buf);

    // Backup header sits in the last LBA.
    let last_lba_off = total.saturating_sub(ss);
    let backup_buf = read_bytes(src, last_lba_off, 512);
    let backup = parse_header(&backup_buf);

    let (hdr, used_backup) = match (&primary, &backup) {
        (Some(p), _) if p.header_crc_ok => (p, false),
        (_, Some(b)) if b.header_crc_ok => {
            notes.push("primary GPT header CRC bad; used backup header".to_string());
            (b, true)
        }
        (Some(p), _) => {
            notes.push("GPT header CRC mismatch (primary); parsing best-effort".to_string());
            (p, false)
        }
        (_, Some(b)) => {
            notes.push("GPT header CRC mismatch (backup); parsing best-effort".to_string());
            (b, true)
        }
        (None, None) => return None,
    };

    let entry_size = hdr.entry_size;
    let num = hdr.num_entries;
    if !(128..=4096).contains(&entry_size) || num == 0 || num > 4096 {
        notes.push(format!(
            "implausible GPT entry geometry (num={num}, size={entry_size}); ignoring"
        ));
        return Some(PartitionMap {
            scheme: Scheme::Gpt,
            entries: Vec::new(),
            confidence: 0.4,
            notes,
            containers: Vec::<Container>::new(),
        });
    }

    let table_off = hdr.entry_lba.saturating_mul(ss);
    let table_bytes = (num as u64).saturating_mul(entry_size as u64).min(1 << 20) as usize;
    let table = read_bytes(src, table_off, table_bytes);

    // Validate the entries CRC (over num*entry_size bytes).
    let entries_crc_ok = crc32(table.get(0..table_bytes).unwrap_or(&table)) == hdr.entries_crc;
    if !entries_crc_ok {
        notes.push("GPT partition-entry CRC mismatch; entries parsed best-effort".to_string());
    }

    let mut entries: Vec<Partition> = Vec::new();
    let mut containers: Vec<Container> = Vec::new();
    let mut idx = 1u32;
    for i in 0..num as usize {
        let base = i * entry_size as usize;
        let Some(rec) = table.get(base..base + entry_size as usize) else {
            break;
        };
        let Some(type_raw) = rec.get(0..16) else {
            continue;
        };
        if is_zero_guid(type_raw) {
            continue;
        }
        let first_lba = le_u64(rec, 32);
        let last_lba = le_u64(rec, 40);
        if last_lba < first_lba {
            continue;
        }
        let start = first_lba.saturating_mul(ss);
        // Clamp to the source like mbr.rs so an entry running past a truncated
        // image still opens (the tail is simply missing).
        let len = (last_lba - first_lba)
            .saturating_add(1)
            .saturating_mul(ss)
            .min(total.saturating_sub(start));
        if start >= total || len == 0 {
            continue;
        }
        let guid = guid_to_string(type_raw);
        let label = type_label(&guid);
        let name = decode_name(rec.get(56..128).unwrap_or(&[]));
        // Note Apple containers for the later phases.
        if label == "Apple APFS" {
            containers.push(Container {
                kind: "apfs-container",
                offset: start,
                evidence: format!("GPT type GUID {guid}"),
            });
        } else if label == "Apple Core Storage" {
            containers.push(Container {
                kind: "core-storage",
                offset: start,
                evidence: format!("GPT type GUID {guid}"),
            });
        }
        entries.push(Partition {
            index: idx,
            start,
            len,
            scheme: Scheme::Gpt,
            type_byte: None,
            type_guid: Some(guid),
            type_label: label.to_string(),
            name,
            fs_hint: gpt_fs_hint(label),
        });
        idx += 1;
    }

    let confidence = match (hdr.header_crc_ok, entries_crc_ok, used_backup) {
        (true, true, false) => 0.99,
        (true, true, true) => 0.9,
        _ => 0.6,
    };

    Some(PartitionMap {
        scheme: Scheme::Gpt,
        entries,
        confidence,
        notes,
        containers,
    })
}

fn decode_name(raw: &[u8]) -> Option<String> {
    let mut units: Vec<u16> = Vec::new();
    let mut i = 0;
    while i + 1 < raw.len() {
        let u = u16::from_le_bytes([
            raw.get(i).copied().unwrap_or(0),
            raw.get(i + 1).copied().unwrap_or(0),
        ]);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    if units.is_empty() {
        return None;
    }
    Some(String::from_utf16_lossy(&units))
}

fn gpt_fs_hint(label: &str) -> Option<&'static str> {
    match label {
        "Apple APFS" => Some("apfs"),
        "Apple HFS+" => Some("hfs+"),
        "Apple Core Storage" => Some("core-storage"),
        "Microsoft Basic Data" => Some("exfat/ntfs/fat"),
        "Linux filesystem" => Some("linux"),
        "EFI System" => Some("fat"),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use reclaim_block::ImageFile;
    use std::io::Write;
    use std::sync::Arc;

    /// Build a tiny GPT disk image with one entry of `type_guid_raw`.
    fn gpt_image(type_guid_raw: [u8; 16], first_lba: u64, last_lba: u64) -> Vec<u8> {
        let ss = 512usize;
        let total_lba = 2048u64;
        let mut img = vec![0u8; ss * total_lba as usize];
        let entry_size = 128u32;
        let num = 128u32;
        let entry_lba = 2u64;

        // Build the entries table.
        let mut table = vec![0u8; (num * entry_size) as usize];
        table[0..16].copy_from_slice(&type_guid_raw);
        // unique guid (16) left zero is fine for a test.
        table[32..40].copy_from_slice(&first_lba.to_le_bytes());
        table[40..48].copy_from_slice(&last_lba.to_le_bytes());
        let entries_crc = crc32(&table);

        // Build the header.
        let mut hdr = vec![0u8; 92];
        hdr[0..8].copy_from_slice(GPT_SIG);
        hdr[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // revision 1.0
        hdr[12..16].copy_from_slice(&92u32.to_le_bytes()); // header size
                                                           // crc at 16..20 zero for now
        hdr[24..32].copy_from_slice(&1u64.to_le_bytes()); // my lba
        hdr[32..40].copy_from_slice(&(total_lba - 1).to_le_bytes()); // alt lba
        hdr[40..48].copy_from_slice(&34u64.to_le_bytes()); // first usable
        hdr[48..56].copy_from_slice(&(total_lba - 34).to_le_bytes()); // last usable
                                                                      // disk guid 56..72 zero
        hdr[72..80].copy_from_slice(&entry_lba.to_le_bytes());
        hdr[80..84].copy_from_slice(&num.to_le_bytes());
        hdr[84..88].copy_from_slice(&entry_size.to_le_bytes());
        hdr[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let hcrc = crc32(&hdr);
        hdr[16..20].copy_from_slice(&hcrc.to_le_bytes());

        // Place primary header at LBA 1, table at LBA 2.
        img[ss..ss + hdr.len()].copy_from_slice(&hdr);
        let toff = entry_lba as usize * ss;
        img[toff..toff + table.len()].copy_from_slice(&table);
        // Protective MBR.
        img[446 + 4] = 0xEE;
        img[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
        img[446 + 12..446 + 16].copy_from_slice(&(total_lba as u32 - 1).to_le_bytes());
        img[510] = 0x55;
        img[511] = 0xAA;
        img
    }

    fn open(bytes: &[u8]) -> Arc<dyn BlockSource> {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        let p = f.path().to_path_buf();
        let s: Arc<dyn BlockSource> = Arc::new(ImageFile::open(&p).unwrap());
        std::mem::forget(f);
        s
    }

    #[test]
    fn parses_apfs_gpt_with_crc() {
        // Apple APFS type GUID 7C3457EF-0000-11AA-AA11-00306543ECAC on disk.
        let apfs = [
            0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43,
            0xEC, 0xAC,
        ];
        let img = gpt_image(apfs, 40, 2000);
        let src = open(&img);
        let m = parse(&src, 512, src.len()).unwrap();
        assert_eq!(m.scheme, Scheme::Gpt);
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].type_label, "Apple APFS");
        assert_eq!(
            m.entries[0].type_guid.as_deref(),
            Some("7C3457EF-0000-11AA-AA11-00306543ECAC")
        );
        assert!(m.confidence > 0.95, "clean CRCs → high confidence");
        assert_eq!(m.containers.len(), 1);
        assert_eq!(m.containers[0].kind, "apfs-container");
    }

    #[test]
    fn entry_past_truncated_image_is_clamped() {
        let apfs = [
            0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43,
            0xEC, 0xAC,
        ];
        // last_lba = u64::MAX would overflow `last - first + 1`; the image is
        // only 2048 sectors so the entry must be clamped, not dropped.
        let img = gpt_image(apfs, 40, u64::MAX);
        let src = open(&img);
        let m = parse(&src, 512, src.len()).unwrap();
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].start, 40 * 512);
        assert_eq!(m.entries[0].len, src.len() - 40 * 512);
    }
}
