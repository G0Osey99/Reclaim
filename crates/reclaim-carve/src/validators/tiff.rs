//! TIFF IFD walker + camera-RAW `Make` refinement (docs/plan/05 §4.2).
//!
//! Follows the IFD chain (and SubIFDs / ExifIFD), computing the maximum extent
//! of all strip/tile/thumbnail offsets to size the file, and reads
//! `Make`/`Model`/`DateTimeOriginal` for naming. One validator covers TIFF and
//! ~15 RAW formats: the EXIF `Make` (plus the DNG-version tag) refines the id.

use super::{Ctx, Verdict};
use crate::bytes::{be_u16, be_u32, le_u16, le_u32};
use crate::metadata::Metadata;
use crate::Validity;

const MAX: u64 = 2 * 1024 * 1024 * 1024;
const MAX_IFDS: u32 = 64;
const MAX_ENTRIES: u16 = 4096;

struct Order {
    be: bool,
}
impl Order {
    fn u16(&self, b: &[u8], o: usize) -> Option<u16> {
        if self.be {
            be_u16(b, o)
        } else {
            le_u16(b, o)
        }
    }
    fn u32(&self, b: &[u8], o: usize) -> Option<u32> {
        if self.be {
            be_u32(b, o)
        } else {
            le_u32(b, o)
        }
    }
}

/// Validate a TIFF/RAW candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 8);
    let ord = match head.get(0..4) {
        Some([0x49, 0x49, 0x2A, 0x00]) => Order { be: false },
        Some([0x4D, 0x4D, 0x00, 0x2A]) => Order { be: true },
        // Olympus/Panasonic TIFF-magic variants still parse as TIFF-LE.
        Some([0x49, 0x49, _, 0x00]) => Order { be: false },
        _ => return Verdict::reject(),
    };
    let avail = ctx.available().min(MAX);
    let read = |off: u64, len: usize| ctx.read(off, len);
    let mut st = Walk::new();
    let first = ord.u32(&head, 4).unwrap_or(8) as u64;
    walk_ifd_chain(&ord, &read, avail, first, 0, &mut st);

    if st.ifds == 0 {
        return Verdict::reject();
    }
    let end = st.max_extent.max(st.ifd_end).min(avail);
    if end < 8 {
        return Verdict::reject();
    }
    let validity = if st.max_extent > avail {
        Validity::Truncated
    } else {
        Validity::Full
    };
    let id = refine(&st);
    let mut meta = Metadata {
        date: st.date.clone(),
        model: st.model.clone(),
        ..Default::default()
    };
    if let (Some(off), Some(len)) = (st.thumb_off, st.thumb_len) {
        meta.thumb_offset = Some(ctx.file_start.saturating_add(off));
        meta.thumb_len = Some(len);
    }
    let score = if validity == Validity::Full { 85 } else { 55 };
    Verdict::accept(end, validity, score)
        .with_format(id)
        .with_meta(meta)
}

/// Parse an in-memory TIFF (EXIF payload) for date/model/thumbnail; `base_abs`
/// is the absolute offset of the TIFF byte 0 so the thumbnail offset is absolute.
#[must_use]
pub fn parse_exif(tiff: &[u8], base_abs: u64) -> Metadata {
    let ord = match tiff.get(0..4) {
        Some([0x49, 0x49, 0x2A, 0x00]) => Order { be: false },
        Some([0x4D, 0x4D, 0x00, 0x2A]) => Order { be: true },
        _ => return Metadata::default(),
    };
    let avail = tiff.len() as u64;
    let read = |off: u64, len: usize| -> Vec<u8> {
        let start = off as usize;
        let end = start.saturating_add(len).min(tiff.len());
        tiff.get(start..end).map(<[u8]>::to_vec).unwrap_or_default()
    };
    let first = ord.u32(tiff, 4).unwrap_or(8) as u64;
    let mut st = Walk::new();
    walk_ifd_chain(&ord, &read, avail, first, 0, &mut st);
    let mut meta = Metadata {
        date: st.date,
        model: st.model,
        ..Default::default()
    };
    if let (Some(off), Some(len)) = (st.thumb_off, st.thumb_len) {
        meta.thumb_offset = Some(base_abs.saturating_add(off));
        meta.thumb_len = Some(len);
    }
    meta
}

struct Walk {
    max_extent: u64,
    ifd_end: u64,
    ifds: u32,
    make: Option<String>,
    model: Option<String>,
    date: Option<String>,
    thumb_off: Option<u64>,
    thumb_len: Option<u64>,
    is_dng: bool,
    /// Total IFD nodes still allowed to visit (bounds SubIFD recursion).
    ifd_budget: u32,
    /// Total value-array elements still allowed to read (bounds hostile TIFFs
    /// with many large arrays — a fuzz-found DoS; see phase-1.md).
    elem_budget: u64,
}
impl Walk {
    fn new() -> Self {
        Walk {
            max_extent: 0,
            ifd_end: 0,
            ifds: 0,
            make: None,
            model: None,
            date: None,
            thumb_off: None,
            thumb_len: None,
            is_dng: false,
            ifd_budget: 4096,
            elem_budget: 2_000_000,
        }
    }
}

fn walk_ifd_chain(
    ord: &Order,
    read: &dyn Fn(u64, usize) -> Vec<u8>,
    avail: u64,
    mut ifd_off: u64,
    depth: u32,
    st: &mut Walk,
) {
    if depth > 4 {
        return;
    }
    for _ in 0..MAX_IFDS {
        if ifd_off == 0 || ifd_off + 2 > avail || st.ifd_budget == 0 {
            return;
        }
        st.ifd_budget -= 1;
        let cnt_b = read(ifd_off, 2);
        let count = ord.u16(&cnt_b, 0).unwrap_or(0);
        if count == 0 || count > MAX_ENTRIES {
            return;
        }
        st.ifds += 1;
        let entries_len = count as usize * 12;
        let entries = read(ifd_off + 2, entries_len + 4);
        let this_end = ifd_off + 2 + entries_len as u64 + 4;
        st.ifd_end = st.ifd_end.max(this_end);

        let mut strip_offs: Vec<u64> = Vec::new();
        let mut strip_counts: Vec<u64> = Vec::new();
        let mut tile_offs: Vec<u64> = Vec::new();
        let mut tile_counts: Vec<u64> = Vec::new();
        let mut jpeg_if: Option<u64> = None;
        let mut jpeg_len: Option<u64> = None;
        let mut sub_ifds: Vec<u64> = Vec::new();
        let mut exif_ifd: Option<u64> = None;
        let mut elem_budget = st.elem_budget;

        for i in 0..count as usize {
            let base = i * 12;
            let tag = ord.u16(&entries, base).unwrap_or(0);
            let typ = ord.u16(&entries, base + 2).unwrap_or(0);
            let vc = ord.u32(&entries, base + 4).unwrap_or(0) as u64;
            let voff = base + 8;
            let tsize = type_size(typ);
            let total = tsize.saturating_mul(vc);
            macro_rules! values {
                () => {
                    read_values(ord, &entries, voff, read, avail, typ, vc, &mut elem_budget)
                };
            }
            match tag {
                271 => st.make = ascii_value(ord, &entries, voff, read, avail, total),
                272 => st.model = ascii_value(ord, &entries, voff, read, avail, total),
                306 | 36867 => {
                    if st.date.is_none() {
                        if let Some(s) = ascii_value(ord, &entries, voff, read, avail, total) {
                            st.date = exif_date(&s);
                        }
                    }
                }
                273 => strip_offs = values!(),
                279 => strip_counts = values!(),
                324 => tile_offs = values!(),
                325 => tile_counts = values!(),
                513 => jpeg_if = values!().first().copied(),
                514 => jpeg_len = values!().first().copied(),
                330 => sub_ifds = values!(),
                34665 => exif_ifd = values!().first().copied(),
                0xC612 => st.is_dng = true,
                _ => {}
            }
        }
        st.elem_budget = elem_budget;

        for (o, c) in strip_offs.iter().zip(strip_counts.iter()) {
            st.max_extent = st.max_extent.max(o.saturating_add(*c));
        }
        for (o, c) in tile_offs.iter().zip(tile_counts.iter()) {
            st.max_extent = st.max_extent.max(o.saturating_add(*c));
        }
        if let (Some(o), Some(l)) = (jpeg_if, jpeg_len) {
            st.max_extent = st.max_extent.max(o.saturating_add(l));
            if st.thumb_off.is_none() {
                st.thumb_off = Some(o);
                st.thumb_len = Some(l);
            }
        }
        for s in sub_ifds.into_iter().take(64) {
            if st.ifd_budget == 0 {
                break;
            }
            walk_ifd_chain(ord, read, avail, s, depth + 1, st);
        }
        if let Some(e) = exif_ifd {
            walk_ifd_chain(ord, read, avail, e, depth + 1, st);
        }

        let next = ord.u32(&entries, entries_len).unwrap_or(0) as u64;
        if next == 0 || next <= ifd_off {
            return;
        }
        ifd_off = next;
    }
}

#[allow(clippy::too_many_arguments)]
fn read_values(
    ord: &Order,
    entries: &[u8],
    voff: usize,
    read: &dyn Fn(u64, usize) -> Vec<u8>,
    avail: u64,
    typ: u16,
    count: u64,
    budget: &mut u64,
) -> Vec<u64> {
    let tsize = type_size(typ);
    let total = tsize.saturating_mul(count);
    // Bound total work across the whole walk against hostile TIFFs.
    let allow = count.min(*budget).min(16384);
    *budget = budget.saturating_sub(allow);
    if allow == 0 {
        return Vec::new();
    }
    let read_cap = tsize.saturating_mul(allow).min(256 * 1024) as usize;
    let bytes: Vec<u8> = if total <= 4 {
        entries.get(voff..voff + 4).unwrap_or(&[]).to_vec()
    } else {
        let off = ord.u32(entries, voff).unwrap_or(0) as u64;
        if off >= avail {
            return Vec::new();
        }
        read(off, read_cap)
    };
    let n = allow as usize;
    let mut out = Vec::with_capacity(n.min(4096));
    for i in 0..n {
        let v = match typ {
            3 => ord.u16(&bytes, i * 2).map(u64::from),
            4 => ord.u32(&bytes, i * 4).map(u64::from),
            1 | 2 | 7 => bytes.get(i).map(|b| u64::from(*b)),
            _ => ord.u32(&bytes, i * 4).map(u64::from),
        };
        match v {
            Some(x) => out.push(x),
            None => break,
        }
    }
    out
}

fn ascii_value(
    ord: &Order,
    entries: &[u8],
    voff: usize,
    read: &dyn Fn(u64, usize) -> Vec<u8>,
    avail: u64,
    total: u64,
) -> Option<String> {
    let bytes = if total <= 4 {
        entries.get(voff..voff + 4)?.to_vec()
    } else {
        let off = ord.u32(entries, voff)? as u64;
        if off >= avail {
            return None;
        }
        read(off, total.min(256) as usize)
    };
    let s: String = bytes
        .iter()
        .take_while(|b| **b != 0)
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .map(|b| *b as char)
        .collect();
    let t = s.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn exif_date(s: &str) -> Option<String> {
    // "YYYY:MM:DD HH:MM:SS" → "YYYY-MM-DD"
    let d = s.get(0..10)?;
    if d.len() == 10 {
        Some(d.replace(':', "-"))
    } else {
        None
    }
}

fn type_size(t: u16) -> u64 {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

fn refine(st: &Walk) -> &'static str {
    if st.is_dng {
        return "raw.dng";
    }
    match st.make.as_deref().map(str::to_ascii_uppercase) {
        Some(m) if m.contains("NIKON") => "raw.nef",
        Some(m) if m.contains("SONY") => "raw.arw",
        Some(m) if m.contains("CANON") => "raw.cr2",
        Some(m) if m.contains("OLYMPUS") || m.contains("OM DIGITAL") => "raw.orf",
        Some(m) if m.contains("PANASONIC") => "raw.rw2",
        Some(m) if m.contains("PENTAX") || m.contains("RICOH") => "raw.pef",
        Some(m) if m.contains("SAMSUNG") => "raw.srw",
        Some(m) if m.contains("HASSELBLAD") => "raw.3fr",
        Some(m) if m.contains("EPSON") => "raw.erf",
        Some(m) if m.contains("KODAK") => "raw.kdc",
        Some(m) if m.contains("FUJI") => "raw.raf",
        _ => "image.tiff",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    // Minimal little-endian TIFF: header + one IFD with one strip.
    fn tiny_tiff() -> Vec<u8> {
        let mut v = vec![0x49, 0x49, 0x2A, 0x00];
        v.extend_from_slice(&8u32.to_le_bytes()); // IFD at 8
                                                  // pad to offset 8 already (header is 8 bytes)
                                                  // IFD: count=2
        v.extend_from_slice(&2u16.to_le_bytes());
        // entry StripOffsets(273) LONG count1 value=64
        v.extend_from_slice(&273u16.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&64u32.to_le_bytes());
        // entry StripByteCounts(279) LONG count1 value=16
        v.extend_from_slice(&279u16.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&16u32.to_le_bytes());
        // next IFD = 0
        v.extend_from_slice(&0u32.to_le_bytes());
        // pad to 64 + 16 bytes strip
        v.resize(80, 0);
        v
    }

    #[test]
    fn computes_extent() {
        let t = tiny_tiff();
        let r = MemReader::new(&t);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("image.tiff").unwrap(),
        };
        let v = validate(&ctx);
        assert!(v.accept);
        assert_eq!(v.len, 80); // strip 64..80
    }
}
