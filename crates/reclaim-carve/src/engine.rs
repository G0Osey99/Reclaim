//! The carve engine (docs/plan/03 §2.3): a single sequential read feeds the
//! matcher and a statistical text pass; candidates are validated, deduped per
//! the container/priority rules of doc 05 §2, and streamed to a sink.

use crate::matcher::{Candidate, Matcher};
use crate::reader::{Reader, SourceReader};
use crate::validators::{self, Ctx};
use crate::{CarvedFile, Validity};
use reclaim_block::{BlockSource, ReadAheadCache};
use reclaim_sigs::{SigDef, Strategy};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Bytes read per sequential step.
const CHUNK: usize = 8 * 1024 * 1024;
/// Fallback carve cap when a format cannot be sized without a validator.
const FALLBACK_CAP: u64 = 1024 * 1024;

/// Options controlling a carve scan.
#[derive(Clone, Debug)]
pub struct CarveOptions {
    /// Byte-granular matching in every block (not just block boundaries).
    pub brute_force: bool,
    /// Block/cluster size for the fast-path alignment gate (0 ⇒ sector size).
    pub block_size: u32,
    /// Restrict to these families (empty ⇒ all).
    pub families: Vec<String>,
    /// Restrict to these signature ids (empty ⇒ all).
    pub sig_ids: Vec<String>,
    /// Emit `Truncated`/`Suspect` results (default true; false ⇒ Full only).
    pub keep_corrupted: bool,
    /// Cap on any single carved file.
    pub max_file_size: u64,
    /// Absolute byte range to scan (None ⇒ whole source).
    pub range: Option<(u64, u64)>,
}

impl Default for CarveOptions {
    fn default() -> Self {
        CarveOptions {
            brute_force: false,
            block_size: 0,
            families: Vec::new(),
            sig_ids: Vec::new(),
            keep_corrupted: true,
            max_file_size: 4 * 1024 * 1024 * 1024,
            range: None,
        }
    }
}

/// Progress report during a scan.
#[derive(Copy, Clone, Debug)]
pub struct CarveProgress {
    /// Current scan cursor (absolute byte offset).
    pub cursor: u64,
    /// Bytes scanned so far.
    pub scanned: u64,
    /// Total bytes to scan.
    pub total: u64,
    /// Results found so far.
    pub found: u64,
}

/// Result of a scan pass.
#[derive(Copy, Clone, Debug)]
pub struct ScanOutcome {
    /// Final cursor (for resume).
    pub cursor: u64,
    /// Number of results emitted.
    pub found: u64,
    /// True if the scan stopped early because `cancel` was set.
    pub interrupted: bool,
    /// Block size used for the alignment gate.
    pub block_size: u32,
}

/// The carve engine.
#[derive(Debug)]
pub struct CarveEngine {
    matcher: Matcher,
}

impl Default for CarveEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CarveEngine {
    /// Build over the full catalog.
    #[must_use]
    pub fn new() -> Self {
        CarveEngine {
            matcher: Matcher::new(),
        }
    }

    /// Infer the block size from the GCD of the first validated header offsets
    /// (docs/plan/03 §2.3), scanning a bounded prefix. Clamped to
    /// `[sector_size, 1 MiB]`; falls back to the sector size.
    #[must_use]
    pub fn infer_block_size(&self, src: &Arc<dyn BlockSource>) -> u32 {
        let sector = src.sector_size().max(1);
        let cache: Arc<dyn BlockSource> = Arc::new(ReadAheadCache::new(Arc::clone(src)));
        let reader = SourceReader::new(Arc::clone(&cache));
        let total = src.len();
        let limit = total.min(128 * 1024 * 1024);
        let mut offsets: Vec<u64> = Vec::new();
        let mut pos: u64 = 0;
        let overlap = reclaim_sigs::MAX_ANCHOR_LEN.saturating_sub(1);
        'outer: while pos < limit {
            let want = ((limit - pos) as usize).min(CHUNK + overlap);
            let buf = reader.read(pos, want);
            let mut starts: Vec<u64> = Vec::new();
            self.matcher.scan_buffer(pos, &buf, |c: Candidate| {
                if (c.file_start.wrapping_sub(pos)) < CHUNK as u64 {
                    starts.push(c.file_start);
                }
            });
            starts.sort_unstable();
            starts.dedup();
            for fs in starts {
                if let Some(cf) = self.identify_one(&reader, fs, &CarveOptions::default(), sector) {
                    if cf.validity == Validity::Full {
                        offsets.push(cf.offset);
                        if offsets.len() >= 12 {
                            break 'outer;
                        }
                    }
                }
            }
            pos = pos.saturating_add(CHUNK as u64);
        }

        if offsets.len() < 3 {
            return sector;
        }
        let mut g = offsets.first().copied().unwrap_or(0);
        for o in &offsets {
            g = crate::bytes::gcd(g, *o);
        }
        let sector64 = u64::from(sector);
        if g < sector64 || g % sector64 != 0 {
            return sector;
        }
        let clamped = g.min(1024 * 1024).max(sector64);
        u32::try_from(clamped).unwrap_or(sector)
    }

    /// Scan `src`, emitting each carved file to `found` and periodic
    /// `progress`. Resumes from `resume_from`; stops early if `cancel` is set.
    pub fn scan(
        &self,
        src: &Arc<dyn BlockSource>,
        opts: &CarveOptions,
        resume_from: u64,
        progress: &mut dyn FnMut(CarveProgress),
        found: &mut dyn FnMut(CarvedFile),
        cancel: &AtomicBool,
    ) -> ScanOutcome {
        let sector = src.sector_size().max(1);
        let block_size = if opts.block_size == 0 {
            sector
        } else {
            opts.block_size
        };
        let cache: Arc<dyn BlockSource> = Arc::new(ReadAheadCache::new(Arc::clone(src)));
        let reader = SourceReader::new(Arc::clone(&cache));

        let (scan_start, scan_end) = match opts.range {
            Some((a, b)) => (a.min(src.len()), b.min(src.len())),
            None => (0, src.len()),
        };
        let start = resume_from.max(scan_start);
        let total = scan_end.saturating_sub(scan_start);
        let overlap = reclaim_sigs::MAX_ANCHOR_LEN.saturating_sub(1);

        let mut pos = start;
        let mut found_count: u64 = 0;
        let mut claims: Vec<(u64, u64)> = Vec::new();
        let mut interrupted = false;

        while pos < scan_end {
            if cancel.load(Ordering::Relaxed) {
                interrupted = true;
                break;
            }
            let want = ((scan_end - pos) as usize).min(CHUNK + overlap);
            let buf = reader.read(pos, want);

            // 1. Anchor matches.
            let mut per_offset: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
            self.matcher.scan_buffer(pos, &buf, |c: Candidate| {
                let in_primary = c.file_start >= pos && (c.file_start - pos) < CHUNK as u64;
                let before = c.file_start < pos; // header-offset outlier (e.g. iso)
                if in_primary || (before && pos == scan_start) {
                    per_offset.entry(c.file_start).or_default().push(c.sig_idx);
                }
            });

            // 2. Statistical text pass at block boundaries.
            self.text_pass(
                &reader,
                pos,
                &buf,
                block_size,
                scan_end,
                opts,
                &mut per_offset,
            );

            // 3. Validate & emit, in offset order.
            for (fs, sig_idxs) in per_offset {
                if suppressed(&claims, fs) {
                    continue;
                }
                if let Some(cf) = self.identify_from(&reader, fs, &sig_idxs, opts, block_size) {
                    if !opts.keep_corrupted && cf.validity != Validity::Full {
                        continue;
                    }
                    // A Full result claims its range against inner matches.
                    if cf.validity == Validity::Full && cf.len > 0 {
                        claims.push((cf.offset, cf.end()));
                    }
                    prune_claims(&mut claims, fs);
                    found_count += 1;
                    found(cf);
                }
            }

            pos = pos.saturating_add(CHUNK as u64);
            progress(CarveProgress {
                cursor: pos.min(scan_end),
                scanned: pos.min(scan_end).saturating_sub(scan_start),
                total,
                found: found_count,
            });
        }

        ScanOutcome {
            cursor: pos.min(scan_end),
            found: found_count,
            interrupted,
            block_size,
        }
    }

    /// Run the statistical text detector at each block boundary within the
    /// chunk and register text candidates at region starts.
    #[allow(clippy::too_many_arguments)]
    fn text_pass(
        &self,
        reader: &dyn Reader,
        base: u64,
        buf: &[u8],
        block_size: u32,
        scan_end: u64,
        opts: &CarveOptions,
        per_offset: &mut BTreeMap<u64, Vec<usize>>,
    ) {
        if !family_allowed("doc", opts)
            && !opts.sig_ids.iter().any(|s| s == "doc.txt")
            && (!opts.families.is_empty() || !opts.sig_ids.is_empty())
        {
            return;
        }
        let text_sig = match sig_index("doc.txt") {
            Some(i) => i,
            None => return,
        };
        // Text files start at cluster boundaries; probe at the cluster size but
        // never finer than 4 KiB, so the pass stays cheap on small sectors.
        let bs = u64::from(block_size.max(512)).max(4096);
        let _ = reader; // previous block is read from the in-memory chunk
        let mut b = base.div_ceil(bs) * bs;
        while b < base + CHUNK as u64 && b < scan_end {
            let rel = (b - base) as usize;
            let sample = buf.get(rel..(rel + 512).min(buf.len())).unwrap_or(&[]);
            if crate::validators::text::looks_like_text(sample) {
                // Region start: the preceding block is not text. Read it from
                // the chunk when available (avoids a syscall per boundary).
                let prev_ok = if b >= base + bs {
                    let pr = (rel).saturating_sub(bs as usize);
                    let prev = buf.get(pr..pr + 512.min(buf.len() - pr)).unwrap_or(&[]);
                    !crate::validators::text::looks_like_text(prev)
                } else {
                    true // chunk boundary: treat as a region start
                };
                if prev_ok {
                    per_offset.entry(b).or_default().push(text_sig);
                }
            }
            b += bs;
        }
    }

    /// Identify the best result at `file_start` from a set of candidate sigs.
    fn identify_from(
        &self,
        reader: &dyn Reader,
        file_start: u64,
        sig_idxs: &[usize],
        opts: &CarveOptions,
        block_size: u32,
    ) -> Option<CarvedFile> {
        let sigs = self.matcher.signatures();
        let aligned = block_size == 0 || file_start.is_multiple_of(u64::from(block_size));

        // Uniform-block gate (non-brute): reject headers inside a uniform fill.
        if !opts.brute_force {
            let head = reader.read(file_start, 64);
            if head
                .first()
                .map(|f| head.iter().all(|b| b == f))
                .unwrap_or(true)
            {
                return None;
            }
        }

        // Group surviving sigs by validator name.
        let mut best: Option<CarvedFile> = None;
        let mut seen_validators: Vec<&str> = Vec::new();
        for &si in sig_idxs {
            let Some(sig) = sigs.get(si) else { continue };
            if !sig_allowed(sig, opts) {
                continue;
            }
            if !opts.brute_force && sig.block_aligned && !aligned {
                continue;
            }
            let vname = sig.validator.unwrap_or("");
            if !vname.is_empty() && seen_validators.contains(&vname) {
                continue; // validator already run for this offset
            }
            if !vname.is_empty() {
                seen_validators.push(vname);
            }
            if let Some(cf) = self.run_one(reader, file_start, sig, opts) {
                best = pick_better(best, cf);
            }
        }
        best
    }

    /// Convenience: identify using every catalog sig anchored at `file_start`
    /// (used by block-size inference and `sigs test`).
    fn identify_one(
        &self,
        reader: &dyn Reader,
        file_start: u64,
        opts: &CarveOptions,
        block_size: u32,
    ) -> Option<CarvedFile> {
        // Re-derive which sigs anchor here by scanning a small local window.
        let window = reader.read(file_start, reclaim_sigs::MAX_HEADER_SPAN.min(64 * 1024));
        let mut sig_idxs: Vec<usize> = Vec::new();
        self.matcher
            .scan_buffer(file_start, &window, |c: Candidate| {
                if c.file_start == file_start {
                    sig_idxs.push(c.sig_idx);
                }
            });
        self.identify_from(reader, file_start, &sig_idxs, opts, block_size)
    }

    /// Identify at an explicit offset for `sigs test` — returns all accepted
    /// results (one per distinct validator), highest score first.
    #[must_use]
    pub fn identify_at(
        &self,
        reader: &dyn Reader,
        file_start: u64,
        max_len: u64,
    ) -> Vec<CarvedFile> {
        let opts = CarveOptions {
            max_file_size: max_len,
            brute_force: true,
            ..Default::default()
        };
        let window = reader.read(file_start, reclaim_sigs::MAX_HEADER_SPAN.min(64 * 1024));
        let mut sig_idxs: Vec<usize> = Vec::new();
        self.matcher
            .scan_buffer(file_start, &window, |c: Candidate| {
                if c.file_start == file_start {
                    sig_idxs.push(c.sig_idx);
                }
            });
        let sigs = self.matcher.signatures();
        let mut out: Vec<CarvedFile> = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for &si in &sig_idxs {
            let Some(sig) = sigs.get(si) else { continue };
            let vname = sig.validator.unwrap_or("");
            if !vname.is_empty() && seen.contains(&vname) {
                continue;
            }
            if !vname.is_empty() {
                seen.push(vname);
            }
            if let Some(cf) = self.run_one(reader, file_start, sig, &opts) {
                out.push(cf);
            }
        }
        out.sort_by(|a, b| b.score.cmp(&a.score).then(b.len.cmp(&a.len)));
        out
    }

    fn run_one(
        &self,
        reader: &dyn Reader,
        file_start: u64,
        sig: &'static SigDef,
        opts: &CarveOptions,
    ) -> Option<CarvedFile> {
        let max_len = reader
            .len()
            .saturating_sub(file_start)
            .min(opts.max_file_size);
        if max_len == 0 {
            return None;
        }
        let ctx = Ctx {
            reader,
            file_start,
            max_len,
            sig,
        };
        let verdict = match sig.validator {
            Some(name) if validators::has_validator(name) => validators::dispatch(name, &ctx),
            _ => fallback(&ctx),
        };
        if !verdict.accept || verdict.len == 0 {
            return None;
        }
        if verdict.len < sig.min_size {
            return None;
        }
        let start = verdict.start_override.unwrap_or(file_start);
        let format = verdict.format.unwrap_or(sig.id);
        let (family, ext) = match reclaim_sigs::by_id(format) {
            Some(s) => (s.family.to_string(), s.primary_ext().to_string()),
            None => (sig.family.to_string(), sig.primary_ext().to_string()),
        };
        let block_aligned = start % u64::from(reader_sector(reader).max(1)) == 0;

        // Taint check: does the recovered range touch a bad/unread sector?
        let (_, suspect_io) = reader.read_flags(start, 512.min(verdict.len as usize));
        let validity = if suspect_io && verdict.validity == Validity::Full {
            Validity::Suspect
        } else {
            verdict.validity
        };

        Some(CarvedFile {
            offset: start,
            len: verdict.len,
            format: format.to_string(),
            family,
            ext,
            validity,
            score: verdict.score,
            meta: verdict.meta,
            block_aligned,
        })
    }
}

/// Fallback extraction for signatures with no registered validator.
fn fallback(ctx: &Ctx) -> validators::Verdict {
    let avail = ctx.available();
    match ctx.sig.strategy {
        Strategy::Footer => {
            if let Some(ft) = ctx.sig.footer {
                let search = ft.search_max.min(avail);
                let mut pos: u64 = 0;
                let win = 1usize << 20;
                while pos < search {
                    let want = ((search - pos) as usize).min(win);
                    let buf = ctx.read(pos, want);
                    if let Some(m) = memchr::memmem::find(&buf, ft.bytes) {
                        let end = pos + m as u64 + ft.bytes.len() as u64;
                        return validators::Verdict::accept(end, Validity::Full, 65);
                    }
                    if want < win {
                        break;
                    }
                    pos += (want - ft.bytes.len().max(1)) as u64;
                }
            }
            validators::Verdict::accept(avail.min(FALLBACK_CAP), Validity::Truncated, 30)
        }
        _ => {
            // Header-only / unsized: a small bounded Suspect carve.
            if ctx.sig.footer_anchored {
                return validators::Verdict::reject();
            }
            validators::Verdict::accept(avail.min(FALLBACK_CAP), Validity::Suspect, 30)
        }
    }
}

fn reader_sector(_reader: &dyn Reader) -> u32 {
    512
}

fn pick_better(cur: Option<CarvedFile>, new: CarvedFile) -> Option<CarvedFile> {
    match cur {
        None => Some(new),
        Some(c) => {
            let better = new.score > c.score || (new.score == c.score && new.len > c.len);
            if better {
                Some(new)
            } else {
                Some(c)
            }
        }
    }
}

fn family_allowed(family: &str, opts: &CarveOptions) -> bool {
    opts.families.is_empty() || opts.families.iter().any(|f| f == family)
}

fn sig_allowed(sig: &SigDef, opts: &CarveOptions) -> bool {
    if !opts.families.is_empty() && !opts.families.iter().any(|f| f == sig.family) {
        return false;
    }
    if !opts.sig_ids.is_empty() && !opts.sig_ids.iter().any(|s| s == sig.id) {
        return false;
    }
    true
}

fn sig_index(id: &str) -> Option<usize> {
    reclaim_sigs::SIGNATURES.iter().position(|s| s.id == id)
}

fn suppressed(claims: &[(u64, u64)], start: u64) -> bool {
    claims.iter().any(|(s, e)| start > *s && start < *e)
}

fn prune_claims(claims: &mut Vec<(u64, u64)>, cursor: u64) {
    claims.retain(|(_, e)| *e > cursor);
    if claims.len() > 64 {
        // Keep only the most recent claims; bounded memory.
        let drop = claims.len() - 64;
        claims.drain(0..drop);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use reclaim_block::ImageFile;
    use std::io::Write;

    fn synthetic_png(size: usize) -> Vec<u8> {
        use crate::crc::crc32;
        fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&(data.len() as u32).to_be_bytes());
            v.extend_from_slice(kind);
            v.extend_from_slice(data);
            let mut body = Vec::from(&kind[..]);
            body.extend_from_slice(data);
            v.extend_from_slice(&crc32(&body).to_be_bytes());
            v
        }
        let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]));
        let pad = size.saturating_sub(v.len() + 24);
        v.extend_from_slice(&chunk(b"IDAT", &vec![7u8; pad]));
        v.extend_from_slice(&chunk(b"IEND", &[]));
        v
    }

    #[test]
    fn carves_pngs_at_cluster_boundaries() {
        // Two PNGs placed at 4096-aligned offsets in a 32 KiB image.
        let mut img = vec![0u8; 32 * 1024];
        let p1 = synthetic_png(1000);
        let p2 = synthetic_png(1500);
        img[4096..4096 + p1.len()].copy_from_slice(&p1);
        img[12288..12288 + p2.len()].copy_from_slice(&p2);

        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&img).unwrap();
        f.flush().unwrap();
        let src: Arc<dyn BlockSource> = Arc::new(ImageFile::open(f.path()).unwrap());

        let engine = CarveEngine::new();
        let opts = CarveOptions {
            block_size: 4096,
            ..Default::default()
        };
        let mut results = Vec::new();
        let cancel = AtomicBool::new(false);
        let outcome = engine.scan(
            &src,
            &opts,
            0,
            &mut |_p| {},
            &mut |cf| results.push(cf),
            &cancel,
        );
        assert!(!outcome.interrupted);
        let pngs: Vec<_> = results.iter().filter(|c| c.format == "image.png").collect();
        assert_eq!(
            pngs.len(),
            2,
            "should carve exactly two PNGs, got {results:?}"
        );
        assert_eq!(pngs[0].offset, 4096);
        assert_eq!(pngs[0].len, p1.len() as u64);
        assert_eq!(pngs[0].validity, Validity::Full);
        assert_eq!(pngs[1].offset, 12288);
        assert_eq!(pngs[1].len, p2.len() as u64);
    }
}
