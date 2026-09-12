//! [`MappedSource`] — a virtual raw image described by an ordered list of
//! [`Segment`]s over a backing [`BlockSource`], with an LRU cache of decoded
//! compressed blocks.
//!
//! Every container format in this module (DMG UDIF, EnCase E01, and the
//! cluster-table formats VMDK/VDI/VHD/VHDX/QCOW2) parses its metadata into a
//! `MappedSource`: the shared `read_at` then resolves any virtual offset to a
//! zero-fill, a raw copy from the backing store, or a decompressed block. This
//! keeps the fiddly, fuzzed decode/caching logic in one place.

use super::codec::{decompress, Codec};
use crate::source::{BlockSource, ReadResult, SectorStatus, SourceId};
use std::sync::{Arc, Mutex};

/// Where one output range's bytes come from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    /// A hole: the range is zeros (sparse cluster / DMG zero-fill / ignore).
    Zero,
    /// Uncompressed bytes at backing offset `pos` (length = the segment's
    /// `out_len`).
    Raw { pos: u64 },
    /// A compressed block: `clen` bytes at backing offset `pos` decode to the
    /// segment's `out_len` bytes with `codec`.
    Compressed { codec: Codec, pos: u64, clen: u64 },
}

/// One contiguous output range and how to materialize it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    /// Output (virtual-image) byte offset where this segment begins.
    pub out_start: u64,
    /// Output length in bytes.
    pub out_len: u64,
    /// Source of the bytes.
    pub piece: Piece,
}

/// Builds a gap-free, sorted [`Segment`] list, coalescing adjacent holes and
/// back-to-back contiguous raw runs so cluster formats don't explode into one
/// segment per cluster.
#[derive(Debug, Default)]
pub(crate) struct SegmentMap {
    segs: Vec<Segment>,
    next_out: u64,
}

impl SegmentMap {
    pub(crate) fn new() -> Self {
        SegmentMap {
            segs: Vec::new(),
            next_out: 0,
        }
    }

    /// Append `len` output bytes described by `piece`. Consecutive zero holes
    /// merge, and consecutive raw runs that are also contiguous in the backing
    /// store merge; compressed blocks are always kept separate.
    pub(crate) fn push(&mut self, len: u64, piece: Piece) {
        if len == 0 {
            return;
        }
        if let Some(last) = self.segs.last_mut() {
            match (last.piece, piece) {
                (Piece::Zero, Piece::Zero) => {
                    last.out_len = last.out_len.saturating_add(len);
                    self.next_out = self.next_out.saturating_add(len);
                    return;
                }
                (Piece::Raw { pos: p0 }, Piece::Raw { pos: p1 })
                    if p0.saturating_add(last.out_len) == p1 =>
                {
                    last.out_len = last.out_len.saturating_add(len);
                    self.next_out = self.next_out.saturating_add(len);
                    return;
                }
                _ => {}
            }
        }
        self.segs.push(Segment {
            out_start: self.next_out,
            out_len: len,
            piece,
        });
        self.next_out = self.next_out.saturating_add(len);
    }

    /// Total output bytes described so far.
    pub(crate) fn out_len(&self) -> u64 {
        self.next_out
    }

    /// Number of segments (for tests / diagnostics).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.segs.len()
    }

    /// Finalize into a [`MappedSource`] of exactly `out_len` bytes (which must
    /// equal [`SegmentMap::out_len`]).
    pub(crate) fn build(
        self,
        backing: Arc<dyn BlockSource>,
        out_len: u64,
        sector_size: u32,
        id: SourceId,
    ) -> MappedSource {
        MappedSource {
            backing,
            segs: self.segs,
            out_len,
            sector_size: sector_size.max(1),
            id,
            cache: Mutex::new(DecodeCache::new(DECODE_CACHE_BLOCKS)),
        }
    }
}

/// How many decoded compressed blocks to keep resident.
const DECODE_CACHE_BLOCKS: usize = 16;

/// A tiny LRU of decoded compressed blocks keyed by segment index.
#[derive(Debug)]
struct DecodeCache {
    cap: usize,
    order: Vec<usize>,
    map: std::collections::HashMap<usize, Arc<Vec<u8>>>,
}

impl DecodeCache {
    fn new(cap: usize) -> Self {
        DecodeCache {
            cap: cap.max(1),
            order: Vec::new(),
            map: std::collections::HashMap::new(),
        }
    }

    fn get(&mut self, key: usize) -> Option<Arc<Vec<u8>>> {
        let v = self.map.get(&key).cloned()?;
        self.touch(key);
        Some(v)
    }

    fn put(&mut self, key: usize, val: Arc<Vec<u8>>) {
        if self.map.insert(key, val).is_none() {
            self.order.push(key);
            while self.order.len() > self.cap {
                let victim = self.order.remove(0);
                if victim != key {
                    self.map.remove(&victim);
                }
            }
        } else {
            self.touch(key);
        }
    }

    fn touch(&mut self, key: usize) {
        if let Some(p) = self.order.iter().position(|k| *k == key) {
            self.order.remove(p);
        }
        self.order.push(key);
    }
}

/// A virtual raw image over a backing [`BlockSource`] (docs/plan/04 §5).
pub struct MappedSource {
    backing: Arc<dyn BlockSource>,
    segs: Vec<Segment>,
    out_len: u64,
    sector_size: u32,
    id: SourceId,
    cache: Mutex<DecodeCache>,
}

impl std::fmt::Debug for MappedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MappedSource")
            .field("out_len", &self.out_len)
            .field("segments", &self.segs.len())
            .field("id", &self.id)
            .finish()
    }
}

impl MappedSource {
    /// Segment index covering output byte `off`, or `None` if past the end.
    fn seg_for(&self, off: u64) -> Option<usize> {
        // Segments are sorted and gap-free; binary-search by range.
        let mut lo = 0usize;
        let mut hi = self.segs.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let s = self.segs.get(mid)?;
            if off < s.out_start {
                hi = mid;
            } else if off >= s.out_start.saturating_add(s.out_len) {
                lo = mid + 1;
            } else {
                return Some(mid);
            }
        }
        None
    }

    /// Materialize a compressed/raw block's decoded bytes (cached for
    /// compressed). Returns `None` on any read/decode failure.
    fn decoded_block(&self, idx: usize, seg: &Segment) -> Option<Arc<Vec<u8>>> {
        match seg.piece {
            Piece::Zero => None, // callers handle zero directly
            Piece::Raw { .. } => None,
            Piece::Compressed { codec, pos, clen } => {
                if let Ok(mut c) = self.cache.lock() {
                    if let Some(hit) = c.get(idx) {
                        return Some(hit);
                    }
                }
                // A compressed block's declared length is an upper bound (the
                // real stream self-terminates); clamp it to what the backing
                // file actually holds so we never request bytes past EOF.
                let avail = self.backing.len().saturating_sub(pos);
                let clen = usize::try_from(clen.min(avail)).ok()?;
                let out_len = usize::try_from(seg.out_len).ok()?;
                let mut comp = vec![0u8; clen];
                // Compressed blocks (qcow2, E01) are not necessarily
                // sector-aligned in the backing file, so read via the aligned
                // superset helper rather than the raw sector-aligned path.
                if !read_backing_unaligned(&self.backing, pos, &mut comp) {
                    return None;
                }
                let raw = decompress(codec, &comp, out_len)?;
                let arc = Arc::new(raw);
                if let Ok(mut c) = self.cache.lock() {
                    c.put(idx, arc.clone());
                }
                Some(arc)
            }
        }
    }

    /// Fill `dst` (which lies wholly inside segment `idx`) starting `within`
    /// bytes into that segment. Returns false on failure (caller marks bad).
    fn fill_from_segment(&self, idx: usize, within: u64, dst: &mut [u8]) -> bool {
        let Some(seg) = self.segs.get(idx) else {
            return false;
        };
        match seg.piece {
            Piece::Zero => {
                dst.fill(0);
                true
            }
            Piece::Raw { pos } => {
                let at = pos.saturating_add(within);
                // Raw bytes are sector-aligned in every format we emit, but the
                // request may not be; read an aligned window and copy out.
                read_backing_unaligned(&self.backing, at, dst)
            }
            Piece::Compressed { .. } => {
                let Some(block) = self.decoded_block(idx, seg) else {
                    return false;
                };
                let within = match usize::try_from(within) {
                    Ok(v) => v,
                    Err(_) => return false,
                };
                let Some(src) = block.get(within..within.saturating_add(dst.len())) else {
                    return false;
                };
                dst.copy_from_slice(src);
                true
            }
        }
    }
}

/// Read `dst.len()` bytes at an arbitrary (possibly unaligned) backing offset by
/// reading an aligned superset and copying out. Returns false only if a sector
/// covering the *needed* range is bad — the aligned tail past the needed bytes
/// (which may run to EOF for a compressed block of unknown exact length) is
/// ignored.
fn read_backing_unaligned(backing: &Arc<dyn BlockSource>, at: u64, dst: &mut [u8]) -> bool {
    if dst.is_empty() {
        return true;
    }
    let ss = u64::from(backing.sector_size().max(1));
    let start = at - (at % ss);
    let end = at.saturating_add(dst.len() as u64);
    // Align the read up to a sector, but never request past the backing EOF:
    // reading exactly to a non-sector-aligned end leaves no bad sectors, whereas
    // reading past it would mark the valid final partial sector bad.
    let end_al = core::cmp::min(end.div_ceil(ss).saturating_mul(ss), backing.len());
    if end > end_al {
        // The needed range itself runs past EOF — a genuine truncation.
        return false;
    }
    let span = match usize::try_from(end_al.saturating_sub(start)) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let mut tmp = vec![0u8; span];
    let r = backing.read_at(start, &mut tmp);
    let first = ((at - start) / ss) as usize;
    let last = ((end - 1 - start) / ss) as usize;
    for i in first..=last {
        if r.status(i) != SectorStatus::Good {
            return false;
        }
    }
    let off = (at - start) as usize;
    let Some(src) = tmp.get(off..off + dst.len()) else {
        return false;
    };
    dst.copy_from_slice(src);
    true
}

impl BlockSource for MappedSource {
    fn len(&self) -> u64 {
        self.out_len
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn physical_sector_size(&self) -> u32 {
        self.sector_size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> ReadResult {
        let ss = u64::from(self.sector_size);
        let sector_count = buf.len().div_ceil(self.sector_size as usize);
        let mut res = ReadResult::good(self.sector_size, sector_count);
        buf.fill(0);
        if buf.is_empty() {
            return res;
        }
        if !offset.is_multiple_of(ss) || offset >= self.out_len {
            for i in 0..sector_count {
                res.mark_bad(i);
            }
            return res;
        }

        // Walk the request range segment by segment.
        let mut done: usize = 0;
        let total = buf.len();
        while done < total {
            let cur = offset.saturating_add(done as u64);
            if cur >= self.out_len {
                mark_bad_from(&mut res, self.sector_size, total, done);
                break;
            }
            let Some(idx) = self.seg_for(cur) else {
                mark_bad_from(&mut res, self.sector_size, total, done);
                break;
            };
            let seg = match self.segs.get(idx) {
                Some(s) => *s,
                None => {
                    mark_bad_from(&mut res, self.sector_size, total, done);
                    break;
                }
            };
            let within = cur - seg.out_start;
            let seg_remain = seg.out_len - within;
            let want = core::cmp::min(seg_remain, (total - done) as u64) as usize;
            let Some(dst) = buf.get_mut(done..done + want) else {
                break;
            };
            if !self.fill_from_segment(idx, within, dst) {
                mark_bad_range(&mut res, self.sector_size, done, want);
            }
            done += want;
        }
        res
    }

    fn id(&self) -> SourceId {
        self.id.clone()
    }
}

/// Mark every sector overlapping request bytes `[start_byte, buf_len)` bad.
fn mark_bad_from(res: &mut ReadResult, sector_size: u32, buf_len: usize, start_byte: usize) {
    mark_bad_range(
        res,
        sector_size,
        start_byte,
        buf_len.saturating_sub(start_byte),
    );
}

/// Mark every sector overlapping request bytes `[start, start+len)` bad.
fn mark_bad_range(res: &mut ReadResult, sector_size: u32, start: usize, len: usize) {
    if len == 0 {
        return;
    }
    let ss = sector_size as usize;
    let first = start / ss;
    let last = start.saturating_add(len).saturating_sub(1) / ss;
    for i in first..=last {
        if i < res.sector_count() {
            res.mark_bad(i);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::memory::MemorySource;
    use std::io::Write;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn coalesces_zero_and_contiguous_raw() {
        let mut m = SegmentMap::new();
        m.push(512, Piece::Zero);
        m.push(512, Piece::Zero);
        m.push(512, Piece::Raw { pos: 0 });
        m.push(512, Piece::Raw { pos: 512 });
        m.push(512, Piece::Raw { pos: 4096 }); // non-contiguous ⇒ new seg
        assert_eq!(m.len(), 3);
        assert_eq!(m.out_len(), 512 * 5);
    }

    #[test]
    fn mixed_map_reads_correctly() {
        // Backing layout: [raw 1024 bytes @0][zlib block @1024]
        let raw: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        let plain: Vec<u8> = (0..2048u32).map(|i| ((i * 7) % 249) as u8).collect();
        let comp = zlib(&plain);
        let mut backing = Vec::new();
        backing.extend_from_slice(&raw);
        let comp_pos = backing.len() as u64;
        backing.extend_from_slice(&comp);

        let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(backing));
        let mut m = SegmentMap::new();
        m.push(1024, Piece::Raw { pos: 0 });
        m.push(2048, Piece::Zero);
        m.push(
            2048,
            Piece::Compressed {
                codec: Codec::Zlib,
                pos: comp_pos,
                clen: comp.len() as u64,
            },
        );
        let out_len = m.out_len();
        let mapped = m.build(src, out_len, 512, SourceId::new("test:mapped"));
        assert_eq!(mapped.len(), 1024 + 2048 + 2048);

        // Read the raw region.
        let mut b = vec![0u8; 1024];
        assert!(mapped.read_at(0, &mut b).all_good());
        assert_eq!(b, raw);

        // Read the zero hole.
        let mut b = vec![0xFFu8; 2048];
        assert!(mapped.read_at(1024, &mut b).all_good());
        assert!(b.iter().all(|x| *x == 0));

        // Read the compressed region (straddling into it from the hole).
        let mut b = vec![0u8; 2048];
        assert!(mapped.read_at(1024 + 2048, &mut b).all_good());
        assert_eq!(b, plain);

        // A read that spans hole→compressed boundary.
        let mut b = vec![0u8; 1024];
        assert!(mapped.read_at(1024 + 2048 - 512, &mut b).all_good());
        assert!(b[..512].iter().all(|x| *x == 0));
        assert_eq!(&b[512..], &plain[..512]);
    }

    #[test]
    fn read_past_end_marks_bad() {
        let src: Arc<dyn BlockSource> = Arc::new(MemorySource::new(vec![1u8; 1024]));
        let mut m = SegmentMap::new();
        m.push(1024, Piece::Raw { pos: 0 });
        let mapped = m.build(src, 1024, 512, SourceId::new("t"));
        let mut b = vec![9u8; 1024];
        let r = mapped.read_at(512, &mut b);
        assert!(r.any_bad());
        assert_eq!(r.status(0), crate::source::SectorStatus::Good);
        assert_eq!(r.status(1), crate::source::SectorStatus::Bad);
    }
}
