//! ISOBMFF box walker (docs/plan/05 §4.1): MP4/MOV/HEIC/CR3/M4A.
//!
//! Walks top-level boxes `[size(4 BE)][type(4)][...]` summing sizes to the
//! logical end. Handles `mdat` before `moov` (camera layout), 64-bit
//! `largesize` (size==1), and size==0 ("to EOF"). The `ftyp` major brand
//! refines the id (mp4/mov/heic/cr3/m4a/braw). The file ends where the next
//! top-level box header stops looking like a box.

use super::{Ctx, Verdict};
use crate::bytes::{be_u32, be_u64};
use crate::metadata::Metadata;
use crate::Validity;

const MAX: u64 = 16 * 1024 * 1024 * 1024;
const MAX_BOXES: u32 = 2_000_000;

/// Validate an ISOBMFF candidate.
#[must_use]
pub fn validate(ctx: &Ctx) -> Verdict {
    let cap = ctx.available().min(MAX);
    if cap < 16 {
        return Verdict::reject();
    }
    let mut pos: u64 = 0;
    let mut saw_ftyp = false;
    let mut saw_moov = false;
    let mut saw_mdat = false;
    let mut brand: Option<[u8; 4]> = None;
    let mut moov_range: Option<(u64, u64)> = None;
    let mut open_ended = false;

    for _ in 0..MAX_BOXES {
        if pos + 8 > cap {
            break;
        }
        let hdr = ctx.read(pos, 16);
        let size32 = be_u32(&hdr, 0).unwrap_or(0) as u64;
        let typ = match hdr.get(4..8) {
            Some(t) => [
                *t.first().unwrap_or(&0),
                *t.get(1).unwrap_or(&0),
                *t.get(2).unwrap_or(&0),
                *t.get(3).unwrap_or(&0),
            ],
            None => break,
        };
        if !is_box_type(&typ) {
            break; // next bytes are not a box → file ended here
        }
        let (box_size, header) = if size32 == 1 {
            (be_u64(&hdr, 8).unwrap_or(0), 16u64)
        } else if size32 == 0 {
            (cap - pos, 8u64) // extends to EOF
        } else {
            (size32, 8u64)
        };
        if box_size < header {
            break;
        }
        let box_end = pos.saturating_add(box_size);
        if box_end > cap {
            // Media ran out mid-box.
            if pos == 0 {
                return Verdict::reject();
            }
            return finalize(
                pos,
                saw_ftyp,
                saw_moov,
                saw_mdat,
                brand,
                Validity::Truncated,
            );
        }

        match &typ {
            // A second top-level `ftyp` is the next file, not a box of ours.
            b"ftyp" if pos > 0 => break,
            b"ftyp" => {
                saw_ftyp = true;
                if brand.is_none() {
                    if let Some(b) = ctx.read(pos + header, 4).get(..4) {
                        brand = Some([
                            *b.first().unwrap_or(&0),
                            *b.get(1).unwrap_or(&0),
                            *b.get(2).unwrap_or(&0),
                            *b.get(3).unwrap_or(&0),
                        ]);
                    }
                }
            }
            b"moov" => {
                saw_moov = true;
                moov_range = Some((pos + header, box_end));
            }
            b"mdat" => saw_mdat = true,
            _ => {}
        }
        pos = box_end;
        if size32 == 0 {
            // "to EOF" has no real end on a raw device: never claim Full.
            open_ended = true;
            break;
        }
    }

    if pos == 0 || (!saw_ftyp && !saw_moov && !saw_mdat) {
        return Verdict::reject();
    }
    let validity = if open_ended {
        Validity::Truncated
    } else {
        Validity::Full
    };
    let mut v = finalize(pos, saw_ftyp, saw_moov, saw_mdat, brand, validity);
    if let Some((s, e)) = moov_range {
        v.meta = mvhd_meta(ctx, s, e);
    }
    v
}

fn finalize(
    len: u64,
    saw_ftyp: bool,
    saw_moov: bool,
    saw_mdat: bool,
    brand: Option<[u8; 4]>,
    validity: Validity,
) -> Verdict {
    let id = refine(brand, saw_moov);
    let base: u8 = if saw_moov && saw_mdat {
        94
    } else if saw_ftyp {
        80
    } else {
        66
    };
    let score = match validity {
        Validity::Full => base,
        Validity::Truncated => base.saturating_sub(35),
        Validity::Suspect => base.saturating_sub(45),
    };
    Verdict::accept(len, validity, score).with_format(id)
}

/// Map the ftyp major brand to a catalog id.
fn refine(brand: Option<[u8; 4]>, saw_moov: bool) -> &'static str {
    match brand {
        Some(b) => match &b {
            b"qt  " => "video.mov",
            b"crx " => "raw.cr3",
            b"braw" => "raw.braw",
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"hevm" | b"hevs"
            | b"mif1" | b"msf1" => "image.heif",
            b"avif" | b"avis" => "image.heif",
            b"M4A " | b"M4B " => "audio.m4a",
            // isom/iso2/mp41/mp42/avc1/dash/3gp*/… all map to mp4.
            _ => "video.mp4",
        },
        None => {
            if saw_moov {
                "video.mov"
            } else {
                "video.mp4"
            }
        }
    }
}

/// A plausible top-level box type: 4 printable ASCII bytes.
fn is_box_type(t: &[u8; 4]) -> bool {
    t.iter().all(|&c| (0x20..=0x7E).contains(&c) || c == 0xA9)
        && t.iter().any(|&c| c.is_ascii_alphanumeric())
}

/// Find `mvhd` under moov and read creation_time → a date string.
fn mvhd_meta(ctx: &Ctx, moov_start: u64, moov_end: u64) -> Metadata {
    let mut meta = Metadata::default();
    // Scan moov's direct children for `mvhd` (bounded).
    let mut pos = moov_start;
    for _ in 0..4096 {
        if pos + 8 > moov_end {
            break;
        }
        let hdr = ctx.read(pos, 8);
        let size = be_u32(&hdr, 0).unwrap_or(0) as u64;
        let typ = hdr.get(4..8).unwrap_or(&[]);
        if size < 8 {
            break;
        }
        if typ == b"mvhd" {
            let body = ctx.read(pos + 8, 20);
            let version = body.first().copied().unwrap_or(0);
            let ctime = if version == 1 {
                be_u64(&body, 4).unwrap_or(0)
            } else {
                u64::from(be_u32(&body, 4).unwrap_or(0))
            };
            if let Some(d) = mac_epoch_to_date(ctime) {
                meta.date = Some(d);
            }
            break;
        }
        pos = pos.saturating_add(size);
    }
    meta
}

/// Convert seconds since 1904-01-01 (QuickTime epoch) to `YYYY-MM-DD`.
fn mac_epoch_to_date(secs: u64) -> Option<String> {
    if secs < 2_082_844_800 {
        return None; // before 1970 → likely unset/bogus
    }
    let unix = secs - 2_082_844_800;
    Some(unix_to_date(unix))
}

/// Days-based civil date from a Unix timestamp (Howard Hinnant's algorithm).
pub(crate) fn unix_to_date(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::reader::MemReader;
    use reclaim_sigs::by_id;

    fn boxx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    /// mdat-first synthetic MP4, like the generator.
    fn synthetic_mp4() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&boxx(b"ftyp", b"isom\x00\x00\x02\x00isomiso2mp41"));
        v.extend_from_slice(&boxx(b"mdat", &vec![0x42u8; 500]));
        let mut mvhd = vec![0u8; 4];
        mvhd.extend_from_slice(&0u32.to_be_bytes()); // creation
        mvhd.extend_from_slice(&[0u8; 12]);
        v.extend_from_slice(&boxx(b"moov", &boxx(b"mvhd", &mvhd)));
        v
    }

    fn ctx_for<'a>(r: &'a MemReader<'a>) -> Ctx<'a> {
        Ctx {
            reader: r,
            file_start: 0,
            max_len: u64::MAX,
            sig: by_id("video.mp4").unwrap(),
        }
    }

    #[test]
    fn walks_mdat_first_to_exact_end() {
        let mp4 = synthetic_mp4();
        let end = mp4.len() as u64;
        let mut data = mp4;
        data.extend_from_slice(&[0u8; 4096]);
        let r = MemReader::new(&data);
        let v = validate(&ctx_for(&r));
        assert!(v.accept);
        assert_eq!(v.validity, Validity::Full);
        assert_eq!(v.len, end);
        assert_eq!(v.format, Some("video.mp4"));
    }

    #[test]
    fn civil_date() {
        assert_eq!(unix_to_date(0), "1970-01-01");
        assert_eq!(unix_to_date(1_767_225_600), "2026-01-01");
    }
}
