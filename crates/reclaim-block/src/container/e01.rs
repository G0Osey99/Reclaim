//! EnCase / EWF evidence files (`.E01`, multi-segment `.E02`…) — docs/plan/04 §5.
//!
//! An EWF segment is a chain of typed sections. The `volume`/`disk` section
//! gives the chunk geometry; `table` sections list per-chunk offsets (top bit =
//! zlib-compressed) into the segment's `sectors` data. Uncompressed chunks are
//! raw sector data (+ a trailing Adler-32 we ignore); compressed chunks are
//! zlib. We build one [`MappedSource`](super::map::MappedSource) per segment and
//! concatenate them.
//!
//! The EWF layout is undocumented; it was cross-checked against the public
//! `libewf` format notes (LGPL — read for understanding only, no code copied,
//! build guide Part 1.4 / doc 11). Validated against a synthesized image in
//! tests; a real-EnCase corpus check needs `ewfacquire` (host tooling item).

use super::codec::Codec;
use super::concat::ConcatSource;
use super::map::{Piece, SegmentMap};
use super::{le_u32, le_u64, read_exact_vec};
use crate::error::BlockError;
use crate::image_file::ImageFile;
use crate::source::{BlockSource, SourceId};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const EVF_SIG: &[u8; 8] = b"EVF\x09\x0d\x0a\xff\x00";
const LVF_SIG: &[u8; 8] = b"LVF\x09\x0d\x0a\xff\x00";
const COMPRESSED_BIT: u32 = 0x8000_0000;
const OFFSET_MASK: u32 = 0x7FFF_FFFF;
const MAX_CHUNKS: u64 = 64 * 1024 * 1024;

pub(crate) fn open(path: &Path) -> Result<Arc<dyn BlockSource>, BlockError> {
    // Collect segment files: the given .E01 then .E02, .E03 …
    let seg_paths = segment_paths(path);
    if seg_paths.is_empty() {
        return Err(BlockError::Container("ewf: no segments".into()));
    }

    let mut geom: Option<Geometry> = None;
    let mut per_segment: Vec<Arc<dyn BlockSource>> = Vec::new();
    let mut total_chunks: u64 = 0;
    let mut remaining_media: Option<u64> = None;

    for seg in &seg_paths {
        let backing: Arc<dyn BlockSource> = Arc::new(ImageFile::open(seg)?);
        let head = read_exact_vec(&backing, 0, 8).unwrap_or_default();
        if head.as_slice() != EVF_SIG && head.as_slice() != LVF_SIG {
            return Err(BlockError::Container("ewf: bad segment signature".into()));
        }
        let parsed = parse_segment(&backing)?;
        if let Some(g) = parsed.geometry {
            geom = Some(g);
            remaining_media.get_or_insert(g.media_size());
        }
        let g = geom.ok_or_else(|| BlockError::Container("ewf: no volume section".into()))?;
        let chunk_out = g.chunk_size();

        // Build this segment's output map from its chunk table(s).
        let mut smap = SegmentMap::new();
        for tbl in &parsed.tables {
            let n = tbl.entries.len();
            for i in 0..n {
                total_chunks += 1;
                if total_chunks > MAX_CHUNKS {
                    return Err(BlockError::Container("ewf: too many chunks".into()));
                }
                let ent = tbl.entries.get(i).copied().unwrap_or(0);
                let compressed = ent & COMPRESSED_BIT != 0;
                let off = tbl.base_offset.saturating_add(u64::from(ent & OFFSET_MASK));
                let rem = remaining_media.unwrap_or(chunk_out);
                let this_out = core::cmp::min(chunk_out, rem);
                if this_out == 0 {
                    break;
                }
                if compressed {
                    // Length up to the next entry's offset (self-terminating zlib).
                    let clen = next_offset(tbl, i, backing.len()).saturating_sub(off);
                    smap.push(
                        this_out,
                        Piece::Compressed {
                            codec: Codec::Zlib,
                            pos: off,
                            clen: clen.max(1),
                        },
                    );
                } else {
                    smap.push(this_out, Piece::Raw { pos: off });
                }
                remaining_media = Some(rem - this_out);
            }
        }
        let out_len = smap.out_len();
        if out_len > 0 {
            let id = SourceId::new(format!("ewf-seg:{}:{}", seg.display(), out_len));
            per_segment.push(Arc::new(smap.build(
                backing,
                out_len,
                g.bytes_per_sector.max(512),
                id,
            )));
        }
    }

    let g = geom.ok_or_else(|| BlockError::Container("ewf: no geometry".into()))?;
    if per_segment.is_empty() {
        return Err(BlockError::Container("ewf: no chunk data".into()));
    }
    if per_segment.len() == 1 {
        if let Some(single) = per_segment.pop() {
            return Ok(single);
        }
    }
    let id = SourceId::new(format!("ewf:{}:{}", path.display(), g.media_size()));
    Ok(Arc::new(ConcatSource::new(per_segment, id)))
}

#[derive(Copy, Clone)]
struct Geometry {
    sectors_per_chunk: u64,
    bytes_per_sector: u32,
    sector_count: u64,
}

impl Geometry {
    fn chunk_size(&self) -> u64 {
        self.sectors_per_chunk
            .saturating_mul(u64::from(self.bytes_per_sector))
    }
    fn media_size(&self) -> u64 {
        self.sector_count
            .saturating_mul(u64::from(self.bytes_per_sector))
    }
}

struct Table {
    base_offset: u64,
    entries: Vec<u32>,
}

struct Parsed {
    geometry: Option<Geometry>,
    tables: Vec<Table>,
}

/// Walk the section chain of one segment file.
fn parse_segment(backing: &Arc<dyn BlockSource>) -> Result<Parsed, BlockError> {
    let mut geometry = None;
    let mut tables = Vec::new();
    let mut total_entries: u64 = 0;
    let mut off = 13u64; // after the 13-byte file header
    let flen = backing.len();
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > 100_000 {
            break;
        }
        let desc = match read_exact_vec(backing, off, 76) {
            Some(d) => d,
            None => break,
        };
        let ty = desc.get(0..16).unwrap_or(&[]);
        let ty = ty.split(|b| *b == 0).next().unwrap_or(&[]);
        let next = le_u64(&desc, 16).unwrap_or(0);
        let size = le_u64(&desc, 24).unwrap_or(0);
        let data_off = off.saturating_add(76);

        match ty {
            b"volume" | b"disk" => {
                if let Some(g) = parse_volume(backing, data_off) {
                    geometry = Some(g);
                }
            }
            b"table" => {
                if let Some(t) = parse_table(backing, data_off, size) {
                    // Cap the cumulative entry count while walking so a chain of
                    // huge tables cannot accumulate before the caller's check.
                    total_entries = total_entries.saturating_add(t.entries.len() as u64);
                    if total_entries > MAX_CHUNKS {
                        return Err(BlockError::Container("ewf: too many chunks".into()));
                    }
                    tables.push(t);
                }
            }
            b"next" | b"done" => break,
            _ => {}
        }
        if next == 0 || next <= off || next >= flen {
            break;
        }
        off = next;
    }
    Ok(Parsed { geometry, tables })
}

fn parse_volume(backing: &Arc<dyn BlockSource>, at: u64) -> Option<Geometry> {
    let d = read_exact_vec(backing, at, 32)?;
    let chunk_count = u64::from(le_u32(&d, 4)?);
    let sectors_per_chunk = u64::from(le_u32(&d, 8)?);
    let bytes_per_sector = le_u32(&d, 12)?;
    // sector_count: 32-bit in classic E01 at offset 16.
    let sector_count = u64::from(le_u32(&d, 16)?);
    if sectors_per_chunk == 0 || bytes_per_sector == 0 {
        return None;
    }
    Some(Geometry {
        sectors_per_chunk,
        bytes_per_sector,
        sector_count: if sector_count == 0 {
            chunk_count.saturating_mul(sectors_per_chunk)
        } else {
            sector_count
        },
    })
}

fn parse_table(backing: &Arc<dyn BlockSource>, at: u64, size: u64) -> Option<Table> {
    let hdr = read_exact_vec(backing, at, 24)?;
    let count = le_u32(&hdr, 0)? as usize;
    if count == 0 || count > 4_000_000 {
        return None;
    }
    let base_offset = le_u64(&hdr, 8).unwrap_or(0);
    let entries_at = at.saturating_add(24);
    let bytes = count.checked_mul(4)?;
    // Bound by the section size when available.
    if size != 0 && u64::from(bytes as u32) > size {
        return None;
    }
    let raw = read_exact_vec(backing, entries_at, bytes)?;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        entries.push(le_u32(&raw, i * 4)?);
    }
    Some(Table {
        base_offset,
        entries,
    })
}

/// Byte offset just past chunk `i` in `tbl` (used to size compressed chunks).
fn next_offset(tbl: &Table, i: usize, seg_len: u64) -> u64 {
    if let Some(next) = tbl.entries.get(i + 1) {
        tbl.base_offset
            .saturating_add(u64::from(next & OFFSET_MASK))
    } else {
        seg_len
    }
}

/// `.E01` → `.E02`, `.E03`, … stopping at the first missing sibling.
fn segment_paths(first: &Path) -> Vec<PathBuf> {
    let mut out = vec![first.to_path_buf()];
    let Some(s) = first.to_str() else {
        return out;
    };
    let Some(dot) = s.rfind('.') else {
        return out;
    };
    let (stem, ext) = (s.get(..dot).unwrap_or(s), s.get(dot + 1..).unwrap_or(""));
    // Extension like E01 / Ex01 — increment the numeric tail.
    let (prefix, num_str): (String, String) = {
        let split = ext
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_digit())
            .last()
            .map(|(i, _)| i);
        match split {
            Some(i) => (
                ext.get(..i).unwrap_or("").to_string(),
                ext.get(i..).unwrap_or("").to_string(),
            ),
            None => return out,
        }
    };
    let width = num_str.len();
    let Ok(mut n) = num_str.parse::<u32>() else {
        return out;
    };
    loop {
        n += 1;
        let cand = PathBuf::from(format!("{stem}.{prefix}{n:0width$}"));
        if cand.is_file() {
            out.push(cand);
        } else {
            break;
        }
        if out.len() > 100_000 {
            break;
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn desc(ty: &str, next: u64, size: u64) -> Vec<u8> {
        let mut d = vec![0u8; 76];
        d[..ty.len()].copy_from_slice(ty.as_bytes());
        d[16..24].copy_from_slice(&next.to_le_bytes());
        d[24..32].copy_from_slice(&size.to_le_bytes());
        d
    }

    #[test]
    fn synthesized_e01_roundtrip() {
        // Geometry: 1 sector/chunk, 512 bytes/sector, 2 chunks → 1024-byte media.
        let chunk0: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        let chunk1: Vec<u8> = (0..512u32).map(|i| ((i * 3) % 249) as u8).collect();
        let c1_comp = zlib(&chunk1);

        // Layout: header(13) | volume desc+data | table desc+data | sectors data
        let mut buf = Vec::new();
        buf.extend_from_slice(EVF_SIG);
        buf.push(0x01);
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // 13 bytes total

        // volume section: descriptor + 32-byte data.
        let vol_off = buf.len() as u64;
        let mut vol_data = vec![0u8; 32];
        vol_data[4..8].copy_from_slice(&2u32.to_le_bytes()); // chunk_count
        vol_data[8..12].copy_from_slice(&1u32.to_le_bytes()); // sectors/chunk
        vol_data[12..16].copy_from_slice(&512u32.to_le_bytes()); // bytes/sector
        vol_data[16..20].copy_from_slice(&2u32.to_le_bytes()); // sector_count
        let table_off = vol_off + 76 + vol_data.len() as u64;
        buf.extend_from_slice(&desc("volume", table_off, 76 + vol_data.len() as u64));
        buf.extend_from_slice(&vol_data);

        // table section: 24-byte header + 2 u32 entries, base_offset = sectors start.
        // Compute sectors_start after the table section.
        let table_hdr_len = 24u64 + 2 * 4;
        let sectors_off = table_off + 76 + table_hdr_len;
        let e0 = 0u32; // chunk0 offset (relative to base), uncompressed
        let e1 = (chunk0.len() as u32) | COMPRESSED_BIT; // chunk1 compressed
        let mut tbl = Vec::new();
        tbl.extend_from_slice(&2u32.to_le_bytes()); // count
        tbl.extend_from_slice(&0u32.to_le_bytes()); // padding
        tbl.extend_from_slice(&sectors_off.to_le_bytes()); // base_offset
        tbl.extend_from_slice(&0u32.to_le_bytes()); // padding
        tbl.extend_from_slice(&0u32.to_le_bytes()); // checksum
        tbl.extend_from_slice(&e0.to_le_bytes());
        tbl.extend_from_slice(&e1.to_le_bytes());
        let sectors_desc_next = sectors_off + chunk0.len() as u64 + c1_comp.len() as u64;
        buf.extend_from_slice(&desc(
            "table",
            table_off + 76 + tbl.len() as u64,
            76 + tbl.len() as u64,
        ));
        buf.extend_from_slice(&tbl);

        // sectors data: chunk0 raw, then chunk1 compressed.
        buf.extend_from_slice(&chunk0);
        buf.extend_from_slice(&c1_comp);

        // done section.
        buf.extend_from_slice(&desc("done", 0, 76));
        let _ = sectors_desc_next;

        let mut tf = tempfile::NamedTempFile::new().unwrap();
        tf.write_all(&buf).unwrap();
        tf.flush().unwrap();

        let src = open(tf.path()).unwrap();
        assert_eq!(src.len(), 1024);
        let mut out = vec![0u8; 1024];
        let r = src.read_at(0, &mut out);
        assert!(r.all_good(), "read had bad sectors");
        assert_eq!(&out[..512], &chunk0[..]);
        assert_eq!(&out[512..], &chunk1[..]);
    }
}
