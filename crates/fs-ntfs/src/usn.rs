//! Best-effort name mining from the change journal (`$UsnJrnl:$J`) and from
//! directory index slack (`$I30` / INDX blocks) — docs/plan/04 §3.3.
//!
//! These recover *names* (and parent refs) for files whose `$FILE_NAME` is
//! otherwise gone. Round 1 uses them to fill in missing names for records we
//! already located; they do not by themselves yield recoverable file content.

use crate::{le_u16, le_u32, le_u64};

/// A name recovered from the change journal or an index block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinedName {
    /// The file name.
    pub name: String,
    /// MFT record number of the file (low 48 bits of the reference).
    pub file_ref: u64,
    /// MFT record number of the parent directory.
    pub parent: u64,
}

/// Parse a `$UsnJrnl:$J` data buffer for USN_RECORD_V2/V3 entries. Bounded; skips
/// unparseable regions by advancing to the next non-zero dword boundary.
#[must_use]
pub fn parse_usn_stream(data: &[u8]) -> Vec<MinedName> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut guard = 0;
    while pos + 60 <= data.len() && guard < 5_000_000 {
        guard += 1;
        let rec_len = le_u32(data, pos) as usize;
        let major = le_u16(data, pos + 4);
        // The journal is sparse at the front (runs of zeros); skip them.
        if rec_len == 0 {
            pos += 8;
            continue;
        }
        if !(56..=4096).contains(&rec_len)
            || (major != 2 && major != 3)
            || pos + rec_len > data.len()
        {
            pos += 8;
            continue;
        }
        let file_ref = le_u64(data, pos + 8) & 0x0000_FFFF_FFFF_FFFF;
        let parent = le_u64(data, pos + 16) & 0x0000_FFFF_FFFF_FFFF;
        let name_len = le_u16(data, pos + 56) as usize;
        let name_off = le_u16(data, pos + 58) as usize;
        if name_len >= 2 && name_off + name_len <= rec_len {
            let name = decode_utf16(data, pos + name_off, name_len / 2);
            if !name.is_empty() {
                out.push(MinedName {
                    name,
                    file_ref,
                    parent,
                });
            }
        }
        pos += rec_len;
        if out.len() > 1_000_000 {
            break;
        }
    }
    out
}

/// Scan an `$INDEX_ALLOCATION` ($I30) buffer of INDX blocks for `$FILE_NAME`
/// names, including entries left in slack after a deletion. Heuristic and
/// bounded — it walks candidate offsets and keeps plausibly-sized names.
#[must_use]
pub fn scan_indx_names(data: &[u8]) -> Vec<MinedName> {
    let mut out = Vec::new();
    let mut base = 0usize;
    // INDX blocks are typically 4 KiB; step through them.
    while base + 24 <= data.len() {
        if data.get(base..base + 4) == Some(b"INDX") {
            // Index entries begin at 0x18 + EntriesOffset; scan the whole block
            // (including slack) for FILE_NAME-shaped keys.
            scan_block_for_names(data, base, (4096).min(data.len() - base), &mut out);
            base += 4096;
        } else {
            base += 512;
        }
        if out.len() > 200_000 {
            break;
        }
    }
    out
}

/// Scan one block region for index entries carrying a `$FILE_NAME` key.
fn scan_block_for_names(data: &[u8], base: usize, len: usize, out: &mut Vec<MinedName>) {
    // An index entry: FileReference(8), Length(2), KeyLength(2), Flags(2),
    // pad(2), then the FILE_NAME key (parent ref 8, …, name_len@0x40,
    // name_type@0x41, name@0x42).
    let region = match data.get(base..base + len) {
        Some(r) => r,
        None => return,
    };
    let mut i = 0usize;
    while i + 0x52 <= region.len() {
        let file_ref = le_u64(region, i) & 0x0000_FFFF_FFFF_FFFF;
        let entry_len = le_u16(region, i + 8) as usize;
        let key_len = le_u16(region, i + 10) as usize;
        // A plausible FILE_NAME key begins at i+16.
        if entry_len >= 0x52 && key_len >= 0x42 && i + 16 + key_len <= region.len() {
            let key = i + 16;
            let parent = le_u64(region, key) & 0x0000_FFFF_FFFF_FFFF;
            let name_len = region.get(key + 0x40).copied().unwrap_or(0) as usize;
            let name_type = region.get(key + 0x41).copied().unwrap_or(0);
            if name_len > 0 && name_len <= 255 && key + 0x42 + name_len * 2 <= region.len() {
                let name = decode_utf16(region, key + 0x42, name_len);
                if !name.is_empty() && name_type != 2 && file_ref != 0 {
                    out.push(MinedName {
                        name,
                        file_ref,
                        parent,
                    });
                }
            }
            i += entry_len.max(8);
        } else {
            i += 8;
        }
    }
}

fn decode_utf16(b: &[u8], off: usize, units: usize) -> String {
    let mut v = Vec::with_capacity(units);
    for i in 0..units.min(255) {
        v.push(le_u16(b, off + i * 2));
    }
    while v.last() == Some(&0) {
        v.pop();
    }
    String::from_utf16_lossy(&v)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_usn_v2_record() {
        let name = "deleted.jpg";
        let name_utf16: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let name_off = 60usize;
        let rec_len = name_off + name_utf16.len();
        let mut buf = vec![0u8; rec_len];
        buf[0..4].copy_from_slice(&(rec_len as u32).to_le_bytes());
        buf[4..6].copy_from_slice(&2u16.to_le_bytes()); // major v2
        buf[8..16].copy_from_slice(&42u64.to_le_bytes()); // file ref
        buf[16..24].copy_from_slice(&5u64.to_le_bytes()); // parent
        buf[56..58].copy_from_slice(&(name_utf16.len() as u16).to_le_bytes());
        buf[58..60].copy_from_slice(&(name_off as u16).to_le_bytes());
        buf[name_off..].copy_from_slice(&name_utf16);

        let recs = parse_usn_stream(&buf);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].name, "deleted.jpg");
        assert_eq!(recs[0].file_ref, 42);
        assert_eq!(recs[0].parent, 5);
    }
}
