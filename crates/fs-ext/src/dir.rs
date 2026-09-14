//! ext directory parsing with deleted-name recovery (docs/plan/04 §2 ext row).
//!
//! A live directory is a linked list of `{inode, rec_len, name_len, file_type,
//! name}` records. On delete the entry's `inode` is often left intact but the
//! previous entry's `rec_len` is extended to "absorb" it, so the deleted record
//! survives in the slack. We walk the live chain and, inside each entry's
//! rec_len slack, scan for plausible stale records to recover deleted names +
//! their inode numbers.

use crate::bytes::{le_u16, le_u32, read};
use crate::inode::Inode;
use crate::sb::Superblock;
use reclaim_block::BlockSource;
use std::sync::Arc;

/// Cap on entries accumulated from one directory (DoS guard on crafted chains).
const MAX_DIR_ENTRIES: usize = 2_000_000;

/// One directory record (live or recovered-deleted).
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub inode: u32,
    pub name: String,
    pub raw_name: Vec<u8>,
    pub file_type: u8,
    pub deleted: bool,
}

/// Parse a directory inode's data blocks into live + recovered-deleted entries.
pub fn read_dir(src: &Arc<dyn BlockSource>, sb: &Superblock, dir: &Inode) -> Vec<DirEntry> {
    let mut out = Vec::new();
    let bs = sb.block_size as usize;
    for ext in dir.extents(src, sb) {
        let mut off = ext.offset;
        let end = ext.offset.saturating_add(ext.len);
        while off < end {
            if out.len() >= MAX_DIR_ENTRIES {
                return out;
            }
            let block = read(src, off, bs);
            parse_dir_block(sb, &block, &mut out);
            off = off.saturating_add(sb.block_size);
        }
    }
    out
}

/// Parse one directory block: live chain + slack recovery.
fn parse_dir_block(sb: &Superblock, block: &[u8], out: &mut Vec<DirEntry>) {
    let len = block.len();
    let mut pos = 0usize;
    let mut guard = 0usize;
    while pos + 8 <= len {
        guard += 1;
        if guard > 100_000 {
            break;
        }
        let inode = le_u32(block, pos);
        let rec_len = le_u16(block, pos + 4) as usize;
        if rec_len < 8 || pos + rec_len > len {
            break; // corrupt chain — stop this block
        }
        let name_len = block.get(pos + 6).copied().unwrap_or(0) as usize;
        let file_type = block.get(pos + 7).copied().unwrap_or(0);
        if inode != 0 && name_len > 0 && pos + 8 + name_len <= len {
            if let Some(name_bytes) = block.get(pos + 8..pos + 8 + name_len) {
                if is_plausible_name(name_bytes) {
                    out.push(DirEntry {
                        inode,
                        name: String::from_utf8_lossy(name_bytes).into_owned(),
                        raw_name: name_bytes.to_vec(),
                        file_type,
                        deleted: false,
                    });
                }
            }
            // Scan this entry's slack for absorbed deleted records.
            let real = round4(8 + name_len);
            let slack_start = pos + real;
            let slack_end = pos + rec_len;
            scan_slack(sb, block, slack_start, slack_end, out);
        } else {
            // An empty/absorbed entry: its whole rec_len is slack.
            scan_slack(sb, block, pos + 8, pos + rec_len, out);
        }
        pos += rec_len;
    }
}

/// Scan `[start, end)` for plausible stale directory records.
fn scan_slack(sb: &Superblock, block: &[u8], start: usize, end: usize, out: &mut Vec<DirEntry>) {
    let end = end.min(block.len());
    let mut p = round4(start);
    while p + 8 <= end {
        let inode = le_u32(block, p);
        let name_len = block.get(p + 6).copied().unwrap_or(0) as usize;
        let file_type = block.get(p + 7).copied().unwrap_or(0);
        if inode != 0
            && inode <= sb.inodes_count
            && name_len > 0
            && name_len <= 255
            && p + 8 + name_len <= end
        {
            if let Some(name_bytes) = block.get(p + 8..p + 8 + name_len) {
                if is_plausible_name(name_bytes) {
                    out.push(DirEntry {
                        inode,
                        name: String::from_utf8_lossy(name_bytes).into_owned(),
                        raw_name: name_bytes.to_vec(),
                        file_type,
                        deleted: true,
                    });
                    p = round4(p + 8 + name_len);
                    continue;
                }
            }
        }
        p += 4; // dirents are 4-byte aligned
    }
}

fn round4(n: usize) -> usize {
    (n + 3) & !3
}

/// A name is plausible if it is non-empty, has no control bytes or '/', and is
/// not the `.`/`..` pseudo-entries (which are never "deleted").
fn is_plausible_name(name: &[u8]) -> bool {
    if name.is_empty() || name == b"." || name == b".." {
        return false;
    }
    name.iter().all(|&c| c >= 0x20 && c != b'/' && c != 0x7f)
}
