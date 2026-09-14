//! HFS+ catalog record parsing (Apple TN1150). All multi-byte fields are
//! big-endian. Bounds-checked readers keep the parser panic-free on the hostile
//! bytes recovered from node slack and the journal (build guide Part 1.4 rule 3).

/// `kHFSPlusFolderRecord`.
pub const REC_FOLDER: u16 = 0x0001;
/// `kHFSPlusFileRecord`.
pub const REC_FILE: u16 = 0x0002;
/// `kHFSPlusFolderThreadRecord`.
pub const REC_FOLDER_THREAD: u16 = 0x0003;
/// `kHFSPlusFileThreadRecord`.
pub const REC_FILE_THREAD: u16 = 0x0004;

/// `kHFSRootFolderID` — the volume root directory CNID.
pub const ROOT_FOLDER_ID: u32 = 2;
/// `kHFSRootParentID` — the parent of the root directory.
pub const ROOT_PARENT_ID: u32 = 1;

/// Read a big-endian `u16` at `off`, or 0 if out of range.
#[must_use]
pub fn be_u16(b: &[u8], off: usize) -> u16 {
    match b.get(off..off + 2) {
        Some(s) => u16::from_be_bytes([
            s.first().copied().unwrap_or(0),
            s.get(1).copied().unwrap_or(0),
        ]),
        None => 0,
    }
}

/// Read a big-endian `u32` at `off`, or 0 if out of range.
#[must_use]
pub fn be_u32(b: &[u8], off: usize) -> u32 {
    match b.get(off..off + 4) {
        Some(s) => {
            let mut a = [0u8; 4];
            a.copy_from_slice(s);
            u32::from_be_bytes(a)
        }
        None => 0,
    }
}

/// Read a big-endian `u64` at `off`, or 0 if out of range.
#[must_use]
pub fn be_u64(b: &[u8], off: usize) -> u64 {
    match b.get(off..off + 8) {
        Some(s) => {
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            u64::from_be_bytes(a)
        }
        None => 0,
    }
}

/// A single fork extent (`HFSPlusExtentDescriptor`): a start block and a run
/// length in allocation blocks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExtentDescriptor {
    /// First allocation block of the run.
    pub start_block: u32,
    /// Length of the run in allocation blocks.
    pub block_count: u32,
}

/// Parse the 8 inline extents of an `HFSPlusForkData` at `off`.
#[must_use]
pub fn parse_extents(b: &[u8], off: usize) -> Vec<ExtentDescriptor> {
    let mut out = Vec::new();
    for i in 0..8 {
        let base = off + i * 8;
        let start = be_u32(b, base);
        let count = be_u32(b, base + 4);
        if count == 0 {
            break;
        }
        out.push(ExtentDescriptor {
            start_block: start,
            block_count: count,
        });
    }
    out
}

/// A parsed `HFSPlusForkData`: logical size and its inline extents.
#[derive(Clone, Debug)]
pub struct ForkData {
    /// Logical fork size in bytes.
    pub logical_size: u64,
    /// Total allocation blocks (from the fork record).
    pub total_blocks: u32,
    /// The inline extent records (up to 8; the rest live in the extents
    /// overflow B-tree).
    pub extents: Vec<ExtentDescriptor>,
}

impl ForkData {
    /// Parse an 80-byte `HFSPlusForkData` at `off`.
    #[must_use]
    pub fn parse(b: &[u8], off: usize) -> ForkData {
        ForkData {
            logical_size: be_u64(b, off),
            total_blocks: be_u32(b, off + 12),
            extents: parse_extents(b, off + 16),
        }
    }
}

/// A parsed catalog key (`HFSPlusCatalogKey`): parent CNID + node name.
#[derive(Clone, Debug)]
pub struct CatalogKey {
    /// `keyLength` (bytes after the length field).
    pub key_length: u16,
    /// Parent directory CNID.
    pub parent_id: u32,
    /// Node name (UTF-8, from decomposed UTF-16BE).
    pub name: String,
    /// Raw UTF-16BE-decoded name bytes.
    pub raw_name: Vec<u8>,
    /// Byte length of the whole key including the 2-byte length field, padded to
    /// an even boundary — the record data starts here.
    pub total_len: usize,
}

/// Parse a catalog key at `off`. Returns `None` if the geometry is implausible.
#[must_use]
pub fn parse_key(b: &[u8], off: usize) -> Option<CatalogKey> {
    let key_length = be_u16(b, off);
    // keyLength = parentID(4) + nameLength(2) + name(2*nameLen); reasonable range.
    if !(6..=0x2fe).contains(&key_length) {
        return None;
    }
    let parent_id = be_u32(b, off + 2);
    let name_len = be_u16(b, off + 6) as usize;
    // TN1150: keyLength == 6 + 2*nameLength exactly, nameLength <= 255.
    if name_len > 255 || key_length as usize != 6 + 2 * name_len {
        return None;
    }
    let name_off = off + 8;
    let mut units: Vec<u16> = Vec::with_capacity(name_len);
    for i in 0..name_len {
        units.push(be_u16(b, name_off + i * 2));
    }
    let name = String::from_utf16_lossy(&units);
    let raw_name = name.clone().into_bytes();
    // total key = 2 (length field) + key_length, padded up to even.
    let total_len = (2 + key_length as usize + 1) & !1;
    Some(CatalogKey {
        key_length,
        parent_id,
        name,
        raw_name,
        total_len,
    })
}

/// The kind and payload of a catalog record following its key.
#[derive(Clone, Debug)]
pub enum CatalogRecord {
    /// A folder: its CNID.
    Folder { cnid: u32 },
    /// A file: CNID, data fork, resource fork, dates.
    File {
        cnid: u32,
        data_fork: ForkData,
        resource_fork: ForkData,
        create_date: u32,
        mod_date: u32,
    },
    /// A thread record: the parent CNID and name of the object this thread
    /// belongs to (used to resolve paths).
    Thread { parent_id: u32, name: String },
}

/// Parse the record body that follows a catalog key at `data_off`.
#[must_use]
pub fn parse_record(b: &[u8], data_off: usize) -> Option<CatalogRecord> {
    let rec_type = be_u16(b, data_off);
    match rec_type {
        REC_FOLDER => {
            // HFSPlusCatalogFolder: recordType u16, flags u16, valence u32, folderID u32
            let cnid = be_u32(b, data_off + 8);
            Some(CatalogRecord::Folder { cnid })
        }
        REC_FILE => {
            // recordType u16, flags u16, reserved1 u32, fileID u32, dates...(20),
            // permissions(16), userInfo(16), finderInfo(16), textEncoding u32,
            // reserved2 u32, dataFork(80) @88, resourceFork(80) @168.
            let cnid = be_u32(b, data_off + 8);
            let create_date = be_u32(b, data_off + 12);
            let mod_date = be_u32(b, data_off + 16);
            let data_fork = ForkData::parse(b, data_off + 88);
            let resource_fork = ForkData::parse(b, data_off + 168);
            Some(CatalogRecord::File {
                cnid,
                data_fork,
                resource_fork,
                create_date,
                mod_date,
            })
        }
        REC_FOLDER_THREAD | REC_FILE_THREAD => {
            // recordType u16, reserved u16, parentID u32, nodeName(HFSUniStr255)
            let parent_id = be_u32(b, data_off + 4);
            let name_len = be_u16(b, data_off + 8) as usize;
            if name_len > 255 {
                return None;
            }
            let mut units = Vec::with_capacity(name_len);
            for i in 0..name_len {
                units.push(be_u16(b, data_off + 10 + i * 2));
            }
            Some(CatalogRecord::Thread {
                parent_id,
                name: String::from_utf16_lossy(&units),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn key_and_file_record() {
        // Build a catalog key for parent=2 name="a.jpg" then a file record.
        let mut buf = vec![0u8; 512];
        let name: Vec<u16> = "a.jpg".encode_utf16().collect();
        let key_len = 6 + name.len() * 2;
        buf[0..2].copy_from_slice(&(key_len as u16).to_be_bytes());
        buf[2..6].copy_from_slice(&2u32.to_be_bytes());
        buf[6..8].copy_from_slice(&(name.len() as u16).to_be_bytes());
        for (i, u) in name.iter().enumerate() {
            buf[8 + i * 2..8 + i * 2 + 2].copy_from_slice(&u.to_be_bytes());
        }
        let key = parse_key(&buf, 0).unwrap();
        assert_eq!(key.parent_id, 2);
        assert_eq!(key.name, "a.jpg");
        let data_off = key.total_len;
        buf[data_off..data_off + 2].copy_from_slice(&REC_FILE.to_be_bytes());
        buf[data_off + 8..data_off + 12].copy_from_slice(&42u32.to_be_bytes()); // fileID
                                                                                // dataFork logicalSize.
        buf[data_off + 88..data_off + 96].copy_from_slice(&12345u64.to_be_bytes());
        buf[data_off + 88 + 16..data_off + 88 + 20].copy_from_slice(&100u32.to_be_bytes()); // start
        buf[data_off + 88 + 20..data_off + 88 + 24].copy_from_slice(&3u32.to_be_bytes()); // count
        match parse_record(&buf, data_off).unwrap() {
            CatalogRecord::File {
                cnid, data_fork, ..
            } => {
                assert_eq!(cnid, 42);
                assert_eq!(data_fork.logical_size, 12345);
                assert_eq!(data_fork.extents.len(), 1);
                assert_eq!(data_fork.extents[0].start_block, 100);
            }
            other => panic!("expected file, got {other:?}"),
        }
    }
}
