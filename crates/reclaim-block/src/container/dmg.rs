//! Apple UDIF disk images (`.dmg`) — docs/plan/04 §5.
//!
//! A DMG ends in a 512-byte `koly` trailer that points at an XML property list
//! describing the image as a set of `blkx` (`mish`) block tables. Each block
//! table tiles a run of output sectors with chunks that are raw, zero-fill, or
//! compressed (`UDZO`=zlib, `UDBZ`=bzip2, `ULFO`=lzfse, `ULMO`=lzma). We slurp
//! the plist, decode every `<data>` blob that begins with the `mish` magic
//! (robust against plist nesting), and lay the chunks into a
//! [`MappedSource`](super::map::MappedSource).
//!
//! Sources: Apple DMG format is undocumented; layout cross-checked against the
//! public descriptions used by `dmg2img` / `libdmg-hfsplus` (read for
//! understanding — those are permissive/BSD, not GPL). No code copied.

use super::codec::Codec;
use super::map::{Piece, SegmentMap};
use super::{be_u32, be_u64, read_exact_vec};
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::{BlockSource, SourceId};
use std::path::Path;
use std::sync::Arc;

const KOLY: &[u8; 4] = b"koly";
const MISH: u32 = 0x6D69_7368; // 'mish'
const SECTOR: u64 = 512;

/// Cap on the XML plist size we will read (metadata, not data): 64 MiB.
const MAX_XML: u64 = 64 * 1024 * 1024;
/// Cap on total chunks across all blkx tables (DoS guard on a crafted plist).
const MAX_CHUNKS: usize = 8_000_000;

/// Open a `.dmg` as its raw guest image.
pub(crate) fn open(path: &Path) -> Result<super::map::MappedSource, BlockError> {
    let img = ImageFile::open(path)?;
    let backing: Arc<dyn BlockSource> = Arc::new(img);
    let len = backing.len();
    if len < SECTOR {
        return Err(BlockError::Container("dmg: file shorter than koly".into()));
    }

    // koly trailer is the final 512 bytes.
    let koly = read_exact_vec(&backing, len - SECTOR, SECTOR as usize)
        .ok_or_else(|| BlockError::Container("dmg: cannot read koly trailer".into()))?;
    if koly.get(0..4) != Some(KOLY) {
        return Err(BlockError::Container("dmg: bad koly magic".into()));
    }
    let data_fork_offset = be_u64(&koly, 24).unwrap_or(0);
    let xml_offset =
        be_u64(&koly, 216).ok_or_else(|| BlockError::Container("dmg: no XMLOffset".into()))?;
    let xml_length =
        be_u64(&koly, 224).ok_or_else(|| BlockError::Container("dmg: no XMLLength".into()))?;
    let sector_count =
        be_u64(&koly, 492).ok_or_else(|| BlockError::Container("dmg: no SectorCount".into()))?;
    let out_len = sector_count.saturating_mul(SECTOR);

    if xml_length == 0 || xml_length > MAX_XML || xml_offset.saturating_add(xml_length) > len {
        return Err(BlockError::Container("dmg: XML plist out of range".into()));
    }
    let xml = read_exact_vec(&backing, xml_offset, xml_length as usize)
        .ok_or_else(|| BlockError::Container("dmg: cannot read XML plist".into()))?;

    // Decode every <data>…</data> blob; keep the ones that are blkx (mish) tables.
    let mut tables: Vec<Vec<u8>> = Vec::new();
    for blob in iter_plist_data(&xml) {
        let bytes = base64_decode(blob);
        if be_u32(&bytes, 0) == Some(MISH) {
            tables.push(bytes);
        }
    }
    if tables.is_empty() {
        return Err(BlockError::Container("dmg: no blkx tables in plist".into()));
    }

    // Order tables by their first output sector.
    tables.sort_by_key(|t| be_u64(t, 8).unwrap_or(u64::MAX));

    let mut smap = SegmentMap::new();
    let mut total_chunks = 0usize;
    for t in &tables {
        let base_sector = be_u64(t, 8).unwrap_or(0);
        let blkx_data_off = be_u64(t, 24).unwrap_or(0);
        let num = be_u32(t, 200).unwrap_or(0) as usize;
        for i in 0..num {
            let base = 204 + i * 40;
            let Some(etype) = be_u32(t, base) else { break };
            let sect_num = be_u64(t, base + 8).unwrap_or(0);
            let sect_cnt = be_u64(t, base + 16).unwrap_or(0);
            let comp_off = be_u64(t, base + 24).unwrap_or(0);
            let comp_len = be_u64(t, base + 32).unwrap_or(0);

            total_chunks += 1;
            if total_chunks > MAX_CHUNKS {
                return Err(BlockError::Container("dmg: too many chunks".into()));
            }
            // Terminator / comment carry no output.
            if etype == 0xFFFF_FFFF || etype == 0x7FFF_FFFE {
                continue;
            }
            let this_out = sect_cnt.saturating_mul(SECTOR);
            if this_out == 0 {
                continue;
            }
            // Place at the expected output offset; fill any gap with zeros so a
            // malformed table can't silently shift the rest of the image.
            let want_out = base_sector.saturating_add(sect_num).saturating_mul(SECTOR);
            if want_out > smap.out_len() {
                smap.push(want_out - smap.out_len(), Piece::Zero);
            }
            let pos = data_fork_offset
                .saturating_add(blkx_data_off)
                .saturating_add(comp_off);
            let piece = match etype {
                0x0000_0000 | 0x0000_0002 => Piece::Zero, // zero-fill / ignore
                0x0000_0001 => Piece::Raw { pos },        // uncompressed
                0x8000_0005 => Piece::Compressed {
                    codec: Codec::Zlib,
                    pos,
                    clen: comp_len,
                },
                0x8000_0006 => Piece::Compressed {
                    codec: Codec::Bzip2,
                    pos,
                    clen: comp_len,
                },
                0x8000_0007 => Piece::Compressed {
                    codec: Codec::Lzfse,
                    pos,
                    clen: comp_len,
                },
                0x8000_0008 => Piece::Compressed {
                    codec: Codec::Lzma,
                    pos,
                    clen: comp_len,
                },
                // 0x80000004 ADC and anything unknown: unreadable ⇒ zeros so the
                // rest of the image still maps; flagged via bad reads would need
                // per-chunk status, out of scope. Treat as a hole.
                _ => Piece::Zero,
            };
            smap.push(this_out, piece);
        }
    }

    // Pad to the koly-declared size if the tables came up short.
    if out_len > smap.out_len() {
        smap.push(out_len - smap.out_len(), Piece::Zero);
    }
    let final_len = if out_len == 0 {
        smap.out_len()
    } else {
        out_len
    };
    let id = SourceId::new(format!("dmg:{}:{}", path.display(), final_len));
    Ok(smap.build(backing, final_len, SECTOR as u32, id))
}

/// Yield each `<data>…</data>` inner slice from a plist (whitespace kept; the
/// base64 decoder ignores it).
fn iter_plist_data(xml: &[u8]) -> Vec<&[u8]> {
    const OPEN: &[u8] = b"<data>";
    const CLOSE: &[u8] = b"</data>";
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + OPEN.len() <= xml.len() {
        if xml.get(i..i + OPEN.len()) == Some(OPEN) {
            let start = i + OPEN.len();
            // Find the matching close tag.
            let mut j = start;
            let mut found = None;
            while j + CLOSE.len() <= xml.len() {
                if xml.get(j..j + CLOSE.len()) == Some(CLOSE) {
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            match found {
                Some(end) => {
                    if let Some(s) = xml.get(start..end) {
                        out.push(s);
                    }
                    i = end + CLOSE.len();
                }
                None => break,
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Minimal, whitespace-tolerant standard base64 decoder (no external crate:
/// the build guide caps new deps at compression crates). Ignores any character
/// outside the base64 alphabet and stops at padding.
fn base64_decode(input: &[u8]) -> Vec<u8> {
    #[inline]
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in input {
        if c == b'=' {
            break;
        }
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn base64_basic() {
        assert_eq!(base64_decode(b"aGVsbG8="), b"hello");
        assert_eq!(base64_decode(b"bWlzaA=="), b"mish");
        // Whitespace tolerance.
        assert_eq!(base64_decode(b"aGVs\n  bG8="), b"hello");
    }

    #[test]
    fn plist_data_extraction() {
        let xml = b"<plist><dict><key>x</key><data>bWlzaA==</data>\
                    <data>\n  aGVsbG8=\n</data></dict></plist>";
        let blobs = iter_plist_data(xml);
        assert_eq!(blobs.len(), 2);
        assert_eq!(base64_decode(blobs[0]), b"mish");
        assert_eq!(base64_decode(blobs[1]), b"hello");
    }
}
