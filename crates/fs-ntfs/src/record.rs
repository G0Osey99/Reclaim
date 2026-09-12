//! MFT record parsing: fix-ups, attribute iteration, data-run decoding and the
//! `$STANDARD_INFORMATION` / `$FILE_NAME` / `$DATA` extractors (docs/plan/04 §3.3).

use crate::date::filetime_date;
use crate::{le_u16, le_u32, le_u64};

/// Attribute type ids we care about.
pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_DATA: u32 = 0x80;
pub const ATTR_BITMAP: u32 = 0xB0;
pub const ATTR_INDEX_ALLOCATION: u32 = 0xA0;
const ATTR_END: u32 = 0xFFFF_FFFF;

const FLAG_COMPRESSED: u16 = 0x0001;
const FLAG_SPARSE: u16 = 0x8000;

const REC_FLAG_IN_USE: u16 = 0x0001;
const REC_FLAG_DIRECTORY: u16 = 0x0002;

/// A decoded NTFS data run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// Logical cluster number of the run start.
    pub lcn: u64,
    /// Run length in clusters.
    pub length: u64,
    /// A sparse run (a hole — no on-disk bytes).
    pub sparse: bool,
}

/// One parsed attribute.
#[derive(Clone, Debug)]
pub struct Attr {
    /// Attribute type id.
    pub type_id: u32,
    /// Attribute name (empty for the unnamed stream).
    pub name: String,
    /// True if the attribute data is non-resident.
    pub resident: bool,
    /// Real (logical) data size.
    pub real_size: u64,
    /// Non-resident data runs.
    pub runs: Vec<Run>,
    /// The data is compressed.
    pub compressed: bool,
    /// The data is sparse.
    pub sparse: bool,
    /// Absolute offset of resident content within the record.
    content_off: usize,
    /// Resident content length.
    content_len: usize,
}

impl Attr {
    /// Resident content bytes (empty for non-resident attributes).
    #[must_use]
    pub fn resident_data(&self, rec: &[u8]) -> Vec<u8> {
        if !self.resident {
            return Vec::new();
        }
        rec.get(self.content_off..self.content_off + self.content_len)
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
    }
}

/// Compact view of the unnamed `$DATA` attribute.
#[derive(Clone, Debug)]
pub struct DataAttr {
    pub resident: bool,
    pub real_size: u64,
    pub runs: Vec<Run>,
    pub compressed: bool,
    pub sparse: bool,
}

/// A parsed MFT `FILE` record.
#[derive(Debug)]
pub struct MftRecord {
    /// The in-use flag (bit 0 of the record flags).
    pub in_use: bool,
    /// The directory flag (bit 1).
    pub is_directory: bool,
    /// All parsed attributes.
    pub attrs: Vec<Attr>,
    file_names: Vec<FileName>,
    std_created: Option<String>,
    std_modified: Option<String>,
}

#[derive(Clone, Debug)]
struct FileName {
    name: String,
    raw: Vec<u8>,
    name_type: u8, // 0 POSIX, 1 Win32, 2 DOS, 3 Win32&DOS
    parent: u64,
}

impl MftRecord {
    /// Parse a record buffer of `record_size` bytes. Applies fix-ups. Returns
    /// `None` if the `FILE` magic is absent or the header is unusable.
    #[must_use]
    pub fn parse(buf: &[u8], record_size: u64) -> Option<MftRecord> {
        if buf.get(0..4) != Some(b"FILE") {
            return None;
        }
        let mut rec = buf.to_vec();
        apply_fixup(&mut rec, record_size as usize);

        let flags = le_u16(&rec, 22);
        let in_use = flags & REC_FLAG_IN_USE != 0;
        let is_directory = flags & REC_FLAG_DIRECTORY != 0;
        let first_attr = le_u16(&rec, 20) as usize;

        let mut attrs = Vec::new();
        let mut file_names = Vec::new();
        let mut std_created = None;
        let mut std_modified = None;

        let mut a = first_attr;
        let mut guard = 0;
        while a + 4 <= rec.len() && guard < 1024 {
            guard += 1;
            let type_id = le_u32(&rec, a);
            if type_id == ATTR_END {
                break;
            }
            let total_len = le_u32(&rec, a + 4) as usize;
            if total_len < 16 || a + total_len > rec.len() {
                break;
            }
            let non_resident = rec.get(a + 8).copied().unwrap_or(0) != 0;
            let name_len = rec.get(a + 9).copied().unwrap_or(0) as usize;
            let name_off = le_u16(&rec, a + 10) as usize;
            let flags = le_u16(&rec, a + 12);
            let attr_name = if name_len > 0 {
                decode_utf16(&rec, a + name_off, name_len)
            } else {
                String::new()
            };

            let mut attr = Attr {
                type_id,
                name: attr_name.clone(),
                resident: !non_resident,
                real_size: 0,
                runs: Vec::new(),
                compressed: flags & FLAG_COMPRESSED != 0,
                sparse: flags & FLAG_SPARSE != 0,
                content_off: 0,
                content_len: 0,
            };

            if non_resident {
                attr.real_size = le_u64(&rec, a + 48);
                let run_off = le_u16(&rec, a + 32) as usize;
                if a + run_off <= rec.len() {
                    attr.runs = decode_runs(rec.get(a + run_off..a + total_len).unwrap_or(&[]));
                }
            } else {
                let clen = le_u32(&rec, a + 16) as usize;
                let coff = le_u16(&rec, a + 20) as usize;
                attr.real_size = clen as u64;
                attr.content_off = a + coff;
                attr.content_len = clen;
            }

            // Extract the well-known attributes we use for recovery.
            match type_id {
                ATTR_STANDARD_INFORMATION if attr.resident => {
                    let c = rec
                        .get(attr.content_off..attr.content_off + attr.content_len)
                        .unwrap_or(&[]);
                    std_created = filetime_date(le_u64(c, 0));
                    std_modified = filetime_date(le_u64(c, 8));
                }
                ATTR_FILE_NAME if attr.resident => {
                    if let Some(fnm) = parse_file_name(&rec, attr.content_off, attr.content_len) {
                        file_names.push(fnm);
                    }
                }
                _ => {}
            }

            attrs.push(attr);
            a += total_len;
        }

        Some(MftRecord {
            in_use,
            is_directory,
            attrs,
            file_names,
            std_created,
            std_modified,
        })
    }

    /// The best file name: prefer Win32 / Win32&DOS over POSIX over DOS.
    #[must_use]
    pub fn best_file_name(&self) -> Option<(String, Vec<u8>, u64)> {
        let rank = |t: u8| match t {
            1 | 3 => 0, // Win32 / Win32&DOS
            0 => 1,     // POSIX
            2 => 2,     // DOS (8.3)
            _ => 3,
        };
        self.file_names
            .iter()
            .filter(|f| !f.name.is_empty())
            .min_by_key(|f| rank(f.name_type))
            .map(|f| (f.name.clone(), f.raw.clone(), f.parent))
    }

    /// The unnamed `$DATA` stream, if any.
    #[must_use]
    pub fn unnamed_data(&self) -> Option<DataAttr> {
        self.attrs
            .iter()
            .find(|a| a.type_id == ATTR_DATA && a.name.is_empty())
            .map(|a| DataAttr {
                resident: a.resident,
                real_size: a.real_size,
                runs: a.runs.clone(),
                compressed: a.compressed,
                sparse: a.sparse,
            })
    }

    /// Absolute offset (within the record) of the unnamed resident `$DATA`.
    #[must_use]
    pub fn resident_data_offset(&self, _rec: &[u8]) -> Option<u64> {
        self.attrs
            .iter()
            .find(|a| a.type_id == ATTR_DATA && a.name.is_empty() && a.resident)
            .map(|a| a.content_off as u64)
    }

    /// `(created, modified)` from `$STANDARD_INFORMATION`.
    #[must_use]
    pub fn std_info_dates(&self) -> (Option<String>, Option<String>) {
        (self.std_created.clone(), self.std_modified.clone())
    }

    /// A named `$DATA` stream (e.g. `$UsnJrnl:$J`), if present.
    #[must_use]
    pub fn named_data(&self, name: &str) -> Option<DataAttr> {
        self.attrs
            .iter()
            .find(|a| a.type_id == ATTR_DATA && a.name == name)
            .map(|a| DataAttr {
                resident: a.resident,
                real_size: a.real_size,
                runs: a.runs.clone(),
                compressed: a.compressed,
                sparse: a.sparse,
            })
    }

    /// The non-resident `$INDEX_ALLOCATION` ($I30) runs of a directory.
    #[must_use]
    pub fn index_allocation(&self) -> Option<DataAttr> {
        self.attrs
            .iter()
            .find(|a| a.type_id == ATTR_INDEX_ALLOCATION && !a.resident)
            .map(|a| DataAttr {
                resident: a.resident,
                real_size: a.real_size,
                runs: a.runs.clone(),
                compressed: a.compressed,
                sparse: a.sparse,
            })
    }
}

/// Apply the NTFS update-sequence (fix-up) array to `rec` in place. The last two
/// bytes of every 512-byte stride are restored from the array (docs/plan/04 §3.3).
fn apply_fixup(rec: &mut [u8], _record_size: usize) {
    let usa_off = le_u16(rec, 4) as usize;
    let usa_count = le_u16(rec, 6) as usize;
    if usa_count < 2 || usa_off + usa_count * 2 > rec.len() {
        return;
    }
    const STRIDE: usize = 512;
    for k in 0..usa_count - 1 {
        let pos = k * STRIDE + STRIDE - 2;
        let src = usa_off + 2 + k * 2;
        if pos + 2 > rec.len() || src + 2 > rec.len() {
            break;
        }
        let b0 = rec.get(src).copied().unwrap_or(0);
        let b1 = rec.get(src + 1).copied().unwrap_or(0);
        if let Some(slot) = rec.get_mut(pos..pos + 2) {
            slot.copy_from_slice(&[b0, b1]);
        }
    }
}

/// Decode NTFS data runs into `Run`s (signed LCN deltas; 0-length offset = hole).
fn decode_runs(runs: &[u8]) -> Vec<Run> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;
    let mut guard = 0;
    while pos < runs.len() && guard < 100_000 {
        guard += 1;
        let header = runs.get(pos).copied().unwrap_or(0);
        if header == 0 {
            break;
        }
        pos += 1;
        let len_size = (header & 0x0F) as usize;
        let off_size = ((header >> 4) & 0x0F) as usize;
        if len_size == 0 || pos + len_size + off_size > runs.len() {
            break;
        }
        let length = read_le_unsigned(runs, pos, len_size);
        pos += len_size;
        if off_size == 0 {
            // Sparse run (hole): no LCN delta.
            out.push(Run {
                lcn: 0,
                length,
                sparse: true,
            });
            continue;
        }
        let delta = read_le_signed(runs, pos, off_size);
        pos += off_size;
        lcn = lcn.wrapping_add(delta);
        if lcn < 0 {
            break;
        }
        out.push(Run {
            lcn: lcn as u64,
            length,
            sparse: false,
        });
        if out.len() > 65_536 {
            break;
        }
    }
    out
}

fn read_le_unsigned(b: &[u8], off: usize, size: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..size.min(8) {
        v |= u64::from(b.get(off + i).copied().unwrap_or(0)) << (8 * i);
    }
    v
}

fn read_le_signed(b: &[u8], off: usize, size: usize) -> i64 {
    let mut v = read_le_unsigned(b, off, size);
    if size > 0 && size < 8 {
        let sign_bit = 1u64 << (size * 8 - 1);
        if v & sign_bit != 0 {
            v |= !((1u64 << (size * 8)) - 1); // sign-extend
        }
    }
    v as i64
}

fn parse_file_name(rec: &[u8], off: usize, len: usize) -> Option<FileName> {
    let c = rec.get(off..off + len)?;
    let parent = le_u64(c, 0) & 0x0000_FFFF_FFFF_FFFF;
    let name_len = c.get(64).copied().unwrap_or(0) as usize;
    let name_type = c.get(65).copied().unwrap_or(0);
    let name = decode_utf16(c, 66, name_len);
    let raw = c
        .get(66..66 + name_len * 2)
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    Some(FileName {
        name,
        raw,
        name_type,
        parent,
    })
}

/// Decode `units` UTF-16LE code units starting at byte `off` in `b`.
fn decode_utf16(b: &[u8], off: usize, units: usize) -> String {
    let mut v = Vec::with_capacity(units);
    for i in 0..units {
        v.push(le_u16(b, off + i * 2));
    }
    // Strip trailing NULs.
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
    fn runs_decode_single() {
        // header 0x21 → len_size=1, off_size=2. length=0x08, delta=0x0034.
        let runs = [0x21, 0x08, 0x34, 0x00, 0x00];
        let d = decode_runs(&runs);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].length, 8);
        assert_eq!(d[0].lcn, 0x34);
        assert!(!d[0].sparse);
    }

    #[test]
    fn runs_decode_negative_delta() {
        // Two runs: +0x20 then -0x10.
        let runs = [0x11, 0x04, 0x20, 0x11, 0x04, 0xF0, 0x00];
        let d = decode_runs(&runs);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].lcn, 0x20);
        assert_eq!(d[1].lcn, 0x20 - 0x10);
    }

    #[test]
    fn runs_decode_sparse() {
        // header 0x01 → len_size=1, off_size=0 → sparse hole of length 5.
        let runs = [0x01, 0x05, 0x00];
        let d = decode_runs(&runs);
        assert_eq!(d.len(), 1);
        assert!(d[0].sparse);
        assert_eq!(d[0].length, 5);
    }
}
