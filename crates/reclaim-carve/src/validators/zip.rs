//! ZIP walker + sub-type refinement (docs/plan/05 §4.4).
//!
//! Walks local file headers to collect member names (which refine docx/xlsx/
//! pptx/odf/epub/iwork/jar/3mf), then the central directory to the End Of
//! Central Directory record, whose `cd_offset + cd_size + 22 + comment` gives
//! the exact archive length. Data descriptors / ZIP64 fall back to a bounded
//! Suspect carve. Member names still refine the id even when sizing is blocked.

use super::{Ctx, Verdict};
use crate::bytes::{le_u16, le_u32};
use crate::Validity;

const LFH: &[u8] = &[0x50, 0x4B, 0x03, 0x04];
const CDH: &[u8] = &[0x50, 0x4B, 0x01, 0x02];
const EOCD: &[u8] = &[0x50, 0x4B, 0x05, 0x06];
const MAX: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ENTRIES: u32 = 200_000;
const FALLBACK_CAP: u64 = 64 * 1024 * 1024;

/// Validate a ZIP candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let head = ctx.read(0, 4);
    if head == EOCD {
        // Empty archive.
        return Verdict::accept(22, Validity::Full, 60).with_format("archive.zip");
    }
    if head != LFH {
        return Verdict::reject();
    }
    let cap = ctx.available().min(MAX);
    let mut names: Vec<String> = Vec::new();
    let mut first_mimetype: Option<String> = None;

    // Walk local file headers.
    let mut pos: u64 = 0;
    let mut blocked = false;
    for i in 0..MAX_ENTRIES {
        let h = ctx.read(pos, 30);
        if h.get(0..4) != Some(LFH) {
            break;
        }
        let flags = le_u16(&h, 6).unwrap_or(0);
        let comp_size = u64::from(le_u32(&h, 18).unwrap_or(0));
        let name_len = le_u16(&h, 26).unwrap_or(0) as u64;
        let extra_len = le_u16(&h, 28).unwrap_or(0) as u64;
        let name = ctx.read(pos + 30, name_len.min(1024) as usize);
        let name_s: String = name.iter().map(|b| *b as char).collect();
        if i == 0 && name_s == "mimetype" {
            let data_off = pos + 30 + name_len + extra_len;
            let mt = ctx.read(data_off, comp_size.min(128) as usize);
            first_mimetype = Some(mt.iter().map(|b| *b as char).collect());
        }
        if names.len() < 64 {
            names.push(name_s);
        }
        // Data descriptor (bit 3) hides the size in the header → cannot advance.
        if flags & 0x08 != 0 || comp_size == 0 && i > 0 {
            blocked = true;
            break;
        }
        pos = pos + 30 + name_len + extra_len + comp_size;
        if pos + 4 > cap {
            blocked = true;
            break;
        }
    }

    let id = refine(&names, first_mimetype.as_deref());

    // If we reached the central directory, walk it to the EOCD for an exact size.
    if !blocked {
        if let Some(total) = size_from_central_dir(ctx, pos, cap) {
            let validity = if total <= cap {
                Validity::Full
            } else {
                Validity::Truncated
            };
            let score = if validity == Validity::Full { 90 } else { 55 };
            return Verdict::accept(total.min(cap), validity, score).with_format(id);
        }
    }
    // Fallback: bounded Suspect carve (exact sizing needs the central directory).
    let len = cap.min(FALLBACK_CAP);
    Verdict::accept(len, Validity::Suspect, 45).with_format(id)
}

/// From the first central-directory header at `pos`, walk to EOCD and return
/// the total archive length.
fn size_from_central_dir(ctx: &Ctx, mut pos: u64, cap: u64) -> Option<u64> {
    let cd_start = pos;
    for _ in 0..MAX_ENTRIES {
        let h = ctx.read(pos, 46);
        match h.get(0..4) {
            Some(sig) if sig == CDH => {
                let name_len = le_u16(&h, 28).unwrap_or(0) as u64;
                let extra_len = le_u16(&h, 30).unwrap_or(0) as u64;
                let comment_len = le_u16(&h, 32).unwrap_or(0) as u64;
                pos = pos + 46 + name_len + extra_len + comment_len;
                if pos > cap {
                    return None;
                }
            }
            Some(sig) if sig == EOCD => {
                let e = ctx.read(pos, 22);
                let comment_len = le_u16(&e, 20).unwrap_or(0) as u64;
                let end = pos + 22 + comment_len;
                let _ = cd_start;
                return Some(end);
            }
            _ => return None,
        }
    }
    None
}

fn refine(names: &[String], mimetype: Option<&str>) -> &'static str {
    if let Some(mt) = mimetype {
        if mt.contains("epub") {
            return "doc.epub";
        }
        if mt.contains("vnd.oasis.opendocument") {
            return "doc.odf";
        }
    }
    let has = |needle: &str| names.iter().any(|n| n.contains(needle));
    let starts = |p: &str| names.iter().any(|n| n.starts_with(p));
    if has("Index/Document.iwa") || starts("Index/") {
        return "doc.iwork";
    }
    if has("3D/3dmodel.model") {
        return "model.3mf";
    }
    if has("[Content_Types].xml") {
        if starts("word/") {
            return "doc.ooxml";
        }
        if starts("xl/") {
            return "doc.ooxml";
        }
        if starts("ppt/") {
            return "doc.ooxml";
        }
        return "doc.ooxml";
    }
    if has("META-INF/MANIFEST.MF") {
        return "archive.zip"; // jar
    }
    "archive.zip"
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    fn local_entry(name: &str, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::from(LFH);
        v.extend_from_slice(&20u16.to_le_bytes()); // version
        v.extend_from_slice(&0u16.to_le_bytes()); // flags (no data descriptor)
        v.extend_from_slice(&0u16.to_le_bytes()); // method: store
        v.extend_from_slice(&0u32.to_le_bytes()); // time/date
        v.extend_from_slice(&0u32.to_le_bytes()); // crc
        v.extend_from_slice(&(data.len() as u32).to_le_bytes()); // comp size
        v.extend_from_slice(&(data.len() as u32).to_le_bytes()); // uncomp size
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes()); // extra len
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(data);
        v
    }

    fn cdh(name: &str, offset: u32) -> Vec<u8> {
        let mut v = Vec::from(CDH);
        v.extend_from_slice(&[0u8; 24]); // versions..sizes (zeroed for test)
        v.extend_from_slice(&(name.len() as u16).to_le_bytes()); // @28 name len
        v.extend_from_slice(&0u16.to_le_bytes()); // extra
        v.extend_from_slice(&0u16.to_le_bytes()); // comment
        v.extend_from_slice(&[0u8; 8]); // disk/attrs
        v.extend_from_slice(&offset.to_le_bytes()); // local header offset
        v.extend_from_slice(name.as_bytes());
        v
    }

    #[test]
    fn epub_refined_and_sized() {
        let mut v = local_entry("mimetype", b"application/epub+zip");
        let e2_off = v.len() as u32;
        v.extend_from_slice(&local_entry("OEBPS/content.opf", b"<xml/>"));
        let cd_off = v.len() as u32;
        v.extend_from_slice(&cdh("mimetype", 0));
        v.extend_from_slice(&cdh("OEBPS/content.opf", e2_off));
        // EOCD
        v.extend_from_slice(EOCD);
        v.extend_from_slice(&[0u8; 8]); // disk numbers (2+2) + entry counts (2+2)
        v.extend_from_slice(&(v.len() as u32 - cd_off).to_le_bytes()); // cd size (approx)
        v.extend_from_slice(&cd_off.to_le_bytes()); // cd offset
        v.extend_from_slice(&0u16.to_le_bytes()); // comment len
        let total = v.len() as u64;
        let r = MemReader::new(&v);
        let ctx = Ctx {
            reader: &r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("archive.zip").unwrap(),
        };
        let out = validate(&ctx);
        assert!(out.accept);
        assert_eq!(out.format, Some("doc.epub"));
        assert_eq!(out.len, total);
    }
}
