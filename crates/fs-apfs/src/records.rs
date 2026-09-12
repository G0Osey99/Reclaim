//! APFS filesystem-record parsing (Apple File System Reference: "File-System
//! Objects"). Records live in the FS tree keyed by `j_key_t`
//! (`obj_id_and_type`: object id in the low 60 bits, record type in the top 4).
//!
//! Reclaim needs five of the record types to recover files:
//! * `APFS_TYPE_INODE` (3) — the inode; its name and logical size arrive as
//!   extended fields (`INO_EXT_TYPE_NAME`, `INO_EXT_TYPE_DSTREAM`);
//! * `APFS_TYPE_DIR_REC` (9) — a directory entry: name (in the key) → child id
//!   and entry kind;
//! * `APFS_TYPE_FILE_EXTENT` (8) — a data extent (logical addr → physical block,
//!   length);
//! * `APFS_TYPE_XATTR` (4) — extended attributes, of which `com.apple.decmpfs`
//!   (transparent compression) and `com.apple.ResourceFork` matter for content.
//!
//! Every field is read with the bounds-checked helpers in [`crate::obj`].

use crate::obj::{le_u16, le_u32, le_u64};

/// `APFS_TYPE_INODE`.
pub const TYPE_INODE: u8 = 3;
/// `APFS_TYPE_XATTR`.
pub const TYPE_XATTR: u8 = 4;
/// `APFS_TYPE_FILE_EXTENT`.
pub const TYPE_FILE_EXTENT: u8 = 8;
/// `APFS_TYPE_DIR_REC`.
pub const TYPE_DIR_REC: u8 = 9;
/// `APFS_TYPE_SNAP_METADATA`.
pub const TYPE_SNAP_METADATA: u8 = 1;

const OBJ_ID_MASK: u64 = (1 << 60) - 1;

/// Extended-field type: file/dir name.
const INO_EXT_TYPE_NAME: u8 = 4;
/// Extended-field type: data stream (`j_dstream`).
const INO_EXT_TYPE_DSTREAM: u8 = 8;

/// Split a `j_key_t` `obj_id_and_type` into (object id, record type).
#[must_use]
pub fn split_key(oat: u64) -> (u64, u8) {
    (oat & OBJ_ID_MASK, (oat >> 60) as u8)
}

/// Directory-entry file kind (from `j_drec_val_t.flags & DREC_TYPE_MASK`,
/// which stores a POSIX `DT_*` value).
#[must_use]
pub fn drec_is_dir(flags: u16) -> bool {
    (flags & 0x0f) == 4 // DT_DIR
}

/// A parsed directory record (`APFS_TYPE_DIR_REC`).
#[derive(Clone, Debug)]
pub struct DirRec {
    /// Parent directory object id (the record's key object id).
    pub parent_id: u64,
    /// Entry name (UTF-8, best effort).
    pub name: String,
    /// Raw name bytes.
    pub raw_name: Vec<u8>,
    /// Child object id (`file_id`).
    pub file_id: u64,
    /// Whether the child is a directory.
    pub is_dir: bool,
    /// `date_added` (APFS nanoseconds since the Unix epoch).
    pub date_added: u64,
}

/// Parse a directory record. `hashed` selects the key layout: volumes with a
/// case- or normalization-insensitive feature use `j_drec_hashed_key_t`
/// (a 32-bit name-length-and-hash), others use `j_drec_key_t` (a 16-bit length).
#[must_use]
pub fn parse_dir_rec(parent_id: u64, key: &[u8], val: &[u8], hashed: bool) -> Option<DirRec> {
    // Key: j_key (8) then either u32 name_len_and_hash + name, or u16 len + name.
    let (name_off, name_len) = if hashed {
        let nlh = le_u32(key, 8);
        (12usize, (nlh & 0x3ff) as usize)
    } else {
        (10usize, le_u16(key, 8) as usize)
    };
    if name_len == 0 || name_len > 1024 {
        return None;
    }
    let raw = key.get(name_off..name_off + name_len)?;
    // Names are NUL-terminated; drop the terminator and anything after it.
    let raw: Vec<u8> = raw.iter().take_while(|&&b| b != 0).copied().collect();
    if raw.is_empty() {
        return None;
    }
    let file_id = le_u64(val, 0);
    let date_added = le_u64(val, 8);
    let flags = le_u16(val, 16);
    Some(DirRec {
        parent_id,
        name: String::from_utf8_lossy(&raw).into_owned(),
        raw_name: raw,
        file_id,
        is_dir: drec_is_dir(flags),
        date_added,
    })
}

/// A parsed inode record (`APFS_TYPE_INODE`) with the fields Reclaim reports.
#[derive(Clone, Debug)]
pub struct Inode {
    /// This inode's object id (the record's key object id).
    pub id: u64,
    /// Parent object id (`j_inode_val_t.parent_id`).
    pub parent_id: u64,
    /// Name from the `INO_EXT_TYPE_NAME` extended field, if present.
    pub name: Option<String>,
    /// Logical size from the `INO_EXT_TYPE_DSTREAM` extended field, if present.
    pub size: Option<u64>,
    /// Creation time (APFS nanoseconds since the Unix epoch).
    pub create_time: u64,
    /// Modification time.
    pub mod_time: u64,
    /// Whether `mode` marks this a directory.
    pub is_dir: bool,
}

/// Fixed portion of `j_inode_val_t` before the extended-field blob.
const INODE_FIXED: usize = 92;

/// Parse an inode value.
#[must_use]
pub fn parse_inode(id: u64, val: &[u8]) -> Inode {
    let parent_id = le_u64(val, 0);
    let create_time = le_u64(val, 16);
    let mod_time = le_u64(val, 24);
    let mode = le_u16(val, 80);
    // S_IFMT = 0xF000, S_IFDIR = 0x4000.
    let is_dir = (mode & 0xf000) == 0x4000;
    let (name, size) = parse_inode_xfields(val);
    Inode {
        id,
        parent_id,
        name,
        size,
        create_time,
        mod_time,
        is_dir,
    }
}

/// Parse the `xf_blob_t` extended fields that follow the fixed inode value,
/// returning `(name, dstream_size)`.
fn parse_inode_xfields(val: &[u8]) -> (Option<String>, Option<u64>) {
    let blob = match val.get(INODE_FIXED..) {
        Some(b) if b.len() >= 4 => b,
        _ => return (None, None),
    };
    let num_exts = le_u16(blob, 0) as usize;
    let _used = le_u16(blob, 2);
    let mut hdr = 4usize; // xf entry table cursor
    let mut data = 4usize + num_exts.saturating_mul(4); // data area cursor
    let mut name = None;
    let mut size = None;
    for _ in 0..num_exts.min(64) {
        let x_type = blob.get(hdr).copied().unwrap_or(0);
        let x_size = le_u16(blob, hdr + 2) as usize;
        hdr += 4;
        let payload = blob.get(data..data.saturating_add(x_size));
        match (x_type, payload) {
            (INO_EXT_TYPE_NAME, Some(p)) => {
                let raw: Vec<u8> = p.iter().take_while(|&&b| b != 0).copied().collect();
                if !raw.is_empty() {
                    name = Some(String::from_utf8_lossy(&raw).into_owned());
                }
            }
            (INO_EXT_TYPE_DSTREAM, Some(p)) => {
                // j_dstream_t: size u64 first.
                size = Some(le_u64(p, 0));
            }
            _ => {}
        }
        // Extended-field payloads are padded up to an 8-byte boundary.
        data = data.saturating_add((x_size + 7) & !7);
    }
    (name, size)
}

/// A parsed file-extent record (`APFS_TYPE_FILE_EXTENT`).
#[derive(Copy, Clone, Debug)]
pub struct FileExtent {
    /// Owning inode object id.
    pub id: u64,
    /// Logical byte offset within the file.
    pub logical: u64,
    /// Extent length in bytes.
    pub len: u64,
    /// Physical block number (container-relative); 0 means a hole.
    pub phys_block: u64,
}

/// Parse a file-extent record.
#[must_use]
pub fn parse_file_extent(id: u64, key: &[u8], val: &[u8]) -> FileExtent {
    let logical = le_u64(key, 8);
    let len_and_flags = le_u64(val, 0);
    let len = len_and_flags & ((1u64 << 56) - 1);
    let phys_block = le_u64(val, 8);
    FileExtent {
        id,
        logical,
        len,
        phys_block,
    }
}

/// Extended-attribute name for transparent compression.
pub const XATTR_DECMPFS: &[u8] = b"com.apple.decmpfs";
/// Extended-attribute name for a resource fork.
pub const XATTR_RESOURCE_FORK: &[u8] = b"com.apple.ResourceFork";

/// The extended-attribute name of an `APFS_TYPE_XATTR` record (from its key).
#[must_use]
pub fn parse_xattr_name(key: &[u8]) -> Option<Vec<u8>> {
    let name_len = le_u16(key, 8) as usize;
    if name_len == 0 || name_len > 1024 {
        return None;
    }
    let raw = key.get(10..10 + name_len)?;
    Some(raw.iter().take_while(|&&b| b != 0).copied().collect())
}

/// Convert an APFS timestamp (nanoseconds since the Unix epoch) to `YYYY-MM-DD`.
#[must_use]
pub fn apfs_date(nanos: u64) -> Option<String> {
    if nanos == 0 {
        return None;
    }
    let secs = nanos / 1_000_000_000;
    crate::civil::date_from_unix(secs as i64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn key_split() {
        let oat = (9u64 << 60) | 1234;
        assert_eq!(split_key(oat), (1234, 9));
    }

    #[test]
    fn dir_rec_hashed() {
        // key: j_key(8) + name_len_and_hash(u32) + "hi\0"
        let mut key = vec![0u8; 8];
        let nlh: u32 = 3; // name_len = 3 (incl NUL)
        key.extend_from_slice(&nlh.to_le_bytes());
        key.extend_from_slice(b"hi\0");
        let mut val = vec![0u8; 18];
        val[0..8].copy_from_slice(&42u64.to_le_bytes()); // file_id
        val[16..18].copy_from_slice(&4u16.to_le_bytes()); // DT_DIR
        let r = parse_dir_rec(7, &key, &val, true).unwrap();
        assert_eq!(r.name, "hi");
        assert_eq!(r.file_id, 42);
        assert!(r.is_dir);
        assert_eq!(r.parent_id, 7);
    }

    #[test]
    fn extent_len_masks_flags() {
        let mut val = vec![0u8; 24];
        let lf = (1u64 << 60) | 16384; // flags in high bits, len in low 56
        val[0..8].copy_from_slice(&lf.to_le_bytes());
        val[8..16].copy_from_slice(&1324u64.to_le_bytes());
        let e = parse_file_extent(30, &[0u8; 16], &val);
        assert_eq!(e.len, 16384);
        assert_eq!(e.phys_block, 1324);
    }
}
