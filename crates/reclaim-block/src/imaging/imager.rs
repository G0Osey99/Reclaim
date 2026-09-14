//! Multi-pass imager (docs/plan/02 FR-IMG-1..5).
//!
//! Chunk-oriented: each logical chunk is read with a large block (pass 1); any
//! bad sectors are retried at sector granularity (pass 2) and, with
//! `reverse_pass`, once more in reverse (pass 3); the finalized chunk is then
//! written (raw/sparse or zstd frame) and hashed. Per-chunk hashes persist in
//! the map so `--resume` skips completed chunks; the whole-image hash and
//! first/last-MiB identity hashes are computed from the finished image.

use crate::bad_block::LbaRange;
use crate::imaging::dest::ImageDest;
use crate::imaging::hash::{hash_bytes, Digest};
use crate::imaging::map::{Compression, HashAlgo, ImageMap};
use crate::imaging::mapped::MappedImage;
use crate::source::{BlockSource, SectorStatus};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Default imaging/hashing chunk (also the pass-1 read block).
pub const DEFAULT_CHUNK: u32 = 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Imaging options.
#[derive(Clone, Debug)]
pub struct ImageOptions {
    /// Imaging/hashing chunk size (rounded to the sector size).
    pub chunk_size: u32,
    /// Per-bad-sector retry count (pass 2).
    pub retries: u32,
    /// Add a reverse retry pass over remaining bad sectors (pass 3).
    pub reverse_pass: bool,
    /// Produce sparse raw output.
    pub sparse: bool,
    /// Output compression.
    pub compression: Compression,
    /// Hash algorithm.
    pub hash_algo: HashAlgo,
    /// Continue an interrupted image from its map.
    pub resume: bool,
}

impl Default for ImageOptions {
    fn default() -> Self {
        ImageOptions {
            chunk_size: DEFAULT_CHUNK,
            retries: 2,
            reverse_pass: false,
            sparse: true,
            compression: Compression::None,
            hash_algo: HashAlgo::Blake3,
            resume: false,
        }
    }
}

/// Progress during imaging.
#[derive(Copy, Clone, Debug)]
pub struct ImageProgress {
    /// Current pass (1 = read, 2 = retry).
    pub pass: u8,
    /// Bytes imaged so far.
    pub done: u64,
    /// Total bytes.
    pub total: u64,
    /// Bad sectors so far.
    pub bad_sectors: u64,
}

/// Outcome of an imaging run.
#[derive(Clone, Debug)]
pub struct ImageOutcome {
    /// True if every chunk was imaged.
    pub complete: bool,
    /// True if any bad sectors were recorded.
    pub had_bad: bool,
    /// Total bad sectors.
    pub bad_sectors: u64,
    /// Whole-image hash (present when complete).
    pub whole_hash: Option<String>,
}

/// Image `src` to `out_path`, writing the sidecar `map_path`.
pub fn image(
    src: &Arc<dyn BlockSource>,
    out_path: &Path,
    map_path: &Path,
    opts: &ImageOptions,
    progress: &mut dyn FnMut(ImageProgress),
    cancel: &AtomicBool,
) -> std::io::Result<ImageOutcome> {
    let sector = src.sector_size().max(1);
    let size = src.len();
    let source_id = src.id().as_str().to_string();

    // Load or create the map. On resume the map is authoritative: a map that
    // exists but cannot be loaded is an error (never silently start over and
    // truncate the partial image), and chunk size / hash / compression are
    // properties of the image on disk, not of this invocation.
    let mut map = if opts.resume && map_path.exists() {
        let m = ImageMap::load(map_path)?;
        if m.source_id != source_id || m.size != size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "resume: map {} is for source {} ({} bytes), not {} ({} bytes)",
                    map_path.display(),
                    m.source_id,
                    m.size,
                    source_id,
                    size
                ),
            ));
        }
        if m.chunk_size == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "resume: map has zero chunk size",
            ));
        }
        m
    } else {
        ImageMap::new(
            source_id.clone(),
            size,
            sector,
            round_up(opts.chunk_size.max(sector), sector),
            opts.hash_algo,
            opts.compression,
        )
    };
    let chunk_size = map.chunk_size;
    let hash_algo = map.hash_algo;
    let compression = map.compression;

    let resuming =
        opts.resume && (!map.frames.is_empty() || map.chunk_hashes.iter().any(Option::is_some));
    let mut dest = if resuming && out_path.exists() {
        ImageDest::open_append(out_path, compression, opts.sparse, map.frames.clone())?
    } else {
        ImageDest::create(out_path, compression, opts.sparse)?
    };

    let num = map.num_chunks();
    let mut bad_total: u64 = map.bad_sectors();
    let mut chunks_since_ckpt = 0u32;

    for i in 0..num {
        if cancel.load(Ordering::Relaxed) {
            map.frames = dest.frames().to_vec();
            let _ = map.save(map_path);
            let _ = dest.sync();
            return Ok(ImageOutcome {
                complete: false,
                had_bad: bad_total > 0,
                bad_sectors: bad_total,
                whole_hash: None,
            });
        }
        if map.is_chunk_done(i) {
            continue;
        }
        let offset = i as u64 * u64::from(chunk_size);
        let len = ((size - offset).min(u64::from(chunk_size))) as usize;
        if len == 0 {
            break;
        }
        let mut buf = vec![0u8; len];
        let rr = src.read_at(offset, &mut buf);

        // Pass 2/3: retry bad sectors.
        if rr.any_bad() {
            let bad: Vec<usize> = rr.bad_sectors().collect();
            let mut still_bad: Vec<usize> = Vec::new();
            for s in &bad {
                if !retry_sector(src, offset, *s, sector, opts.retries, &mut buf) {
                    still_bad.push(*s);
                }
            }
            if opts.reverse_pass {
                let mut rem = std::mem::take(&mut still_bad);
                rem.reverse();
                for s in rem {
                    if !retry_sector(src, offset, s, sector, 1, &mut buf) {
                        still_bad.push(s);
                    }
                }
            }
            for s in still_bad {
                let lba = offset / u64::from(sector) + s as u64;
                map.bad.push(LbaRange {
                    start: lba,
                    count: 1,
                });
                bad_total += 1;
            }
        }

        if let Some(slot) = map.chunk_hashes.get_mut(i) {
            *slot = Some(hash_bytes(hash_algo, &buf));
        }
        dest.write_chunk(offset, &buf)?;

        chunks_since_ckpt += 1;
        if chunks_since_ckpt >= 64 {
            map.frames = dest.frames().to_vec();
            coalesce(&mut map.bad);
            let _ = map.save(map_path);
            chunks_since_ckpt = 0;
        }
        progress(ImageProgress {
            pass: 1,
            done: (offset + len as u64).min(size),
            total: size,
            bad_sectors: bad_total,
        });
    }

    // Finalize.
    dest.set_logical_len(size)?;
    map.frames = dest.frames().to_vec();
    dest.sync()?;
    coalesce(&mut map.bad);
    map.complete = map.chunk_hashes.iter().all(Option::is_some);

    if map.complete {
        let (whole, first_mib, last_mib) = compute_identity(out_path, &map)?;
        map.whole_hash = Some(whole.clone());
        map.first_mib_hash = Some(first_mib);
        map.last_mib_hash = Some(last_mib);
        map.save(map_path)?;
        Ok(ImageOutcome {
            complete: true,
            had_bad: bad_total > 0,
            bad_sectors: bad_total,
            whole_hash: Some(whole),
        })
    } else {
        map.save(map_path)?;
        Ok(ImageOutcome {
            complete: false,
            had_bad: bad_total > 0,
            bad_sectors: bad_total,
            whole_hash: None,
        })
    }
}

/// Retry one sector up to `retries` times; on success patch `buf` and return true.
fn retry_sector(
    src: &Arc<dyn BlockSource>,
    chunk_offset: u64,
    sector_idx: usize,
    sector: u32,
    retries: u32,
    buf: &mut [u8],
) -> bool {
    let ss = sector as usize;
    let sub_off = chunk_offset + (sector_idx as u64) * u64::from(sector);
    let mut sub = vec![0u8; ss];
    for _ in 0..retries.max(1) {
        let r = src.read_at(sub_off, &mut sub);
        if r.status(0) == SectorStatus::Good {
            let start = sector_idx * ss;
            let end = (start + ss).min(buf.len());
            if let (Some(dst), Some(s)) = (buf.get_mut(start..end), sub.get(..end - start)) {
                dst.copy_from_slice(s);
            }
            return true;
        }
    }
    false
}

/// Compute whole-image + first/last-MiB hashes from the finished image.
fn compute_identity(out_path: &Path, map: &ImageMap) -> std::io::Result<(String, String, String)> {
    let mi = MappedImage::open(out_path, map.clone())?;
    let src: Arc<dyn BlockSource> = Arc::new(mi);
    let size = map.size;
    let mut whole = Digest::new(map.hash_algo);
    let step = 4 * MIB;
    let mut off = 0u64;
    let mut first_mib: Vec<u8> = Vec::new();
    let mut last_mib_buf: Vec<u8> = Vec::new();
    while off < size {
        let n = ((size - off).min(step)) as usize;
        let mut b = vec![0u8; n];
        src.read_at(off, &mut b);
        whole.update(&b);
        if off < MIB {
            let take = (MIB - off).min(n as u64) as usize;
            first_mib.extend_from_slice(b.get(..take).unwrap_or(&b));
        }
        off += n as u64;
    }
    // Last MiB (aligned down to a sector boundary for the read).
    let ss = u64::from(map.sector_size.max(1));
    let last_start = (size.saturating_sub(MIB)) / ss * ss;
    let n = (size - last_start) as usize;
    if n > 0 {
        let mut b = vec![0u8; n];
        src.read_at(last_start, &mut b);
        last_mib_buf = b;
    }
    Ok((
        whole.finalize_hex(),
        hash_bytes(map.hash_algo, &first_mib),
        hash_bytes(map.hash_algo, &last_mib_buf),
    ))
}

fn round_up(v: u32, m: u32) -> u32 {
    let m = m.max(1);
    v.div_ceil(m).saturating_mul(m)
}

pub(crate) fn coalesce(runs: &mut Vec<LbaRange>) {
    if runs.len() < 2 {
        return;
    }
    runs.sort_by_key(|r| r.start);
    let mut merged: Vec<LbaRange> = Vec::with_capacity(runs.len());
    for r in runs.iter().copied() {
        match merged.last_mut() {
            Some(last) if r.start <= last.end() => {
                let new_end = last.end().max(r.end());
                last.count = new_end - last.start;
            }
            _ => merged.push(r),
        }
    }
    *runs = merged;
}

/// The result of `verify-image`.
#[derive(Clone, Debug)]
pub struct VerifyReport {
    /// Whether the whole-image hash matches the map.
    pub whole_ok: bool,
    /// Number of chunks whose hash mismatched.
    pub chunk_mismatches: usize,
    /// Number of chunks checked.
    pub chunks_checked: usize,
    /// Bad sectors recorded in the map.
    pub bad_sectors: u64,
}

/// Verify an image against its map: recompute per-chunk and whole hashes.
pub fn verify_image(out_path: &Path, map_path: &Path) -> std::io::Result<VerifyReport> {
    let map = ImageMap::load(map_path)?;
    let mi = MappedImage::open(out_path, map.clone())?;
    let src: Arc<dyn BlockSource> = Arc::new(mi);
    let cs = u64::from(map.chunk_size.max(1));
    let mut mismatches = 0usize;
    let mut checked = 0usize;
    let mut whole = Digest::new(map.hash_algo);

    for (i, expect) in map.chunk_hashes.iter().enumerate() {
        let off = i as u64 * cs;
        if off >= map.size {
            break;
        }
        let n = ((map.size - off).min(cs)) as usize;
        let mut b = vec![0u8; n];
        src.read_at(off, &mut b);
        whole.update(&b);
        if let Some(exp) = expect {
            checked += 1;
            if &hash_bytes(map.hash_algo, &b) != exp {
                mismatches += 1;
            }
        }
    }
    let whole_ok = match &map.whole_hash {
        Some(h) => &whole.finalize_hex() == h,
        None => false,
    };
    Ok(VerifyReport {
        whole_ok,
        chunk_mismatches: mismatches,
        chunks_checked: checked,
        bad_sectors: map.bad_sectors(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::fault::FaultInjector;
    use crate::image_file::ImageFile;

    fn make_src(bytes: &[u8]) -> (tempfile::NamedTempFile, Arc<dyn BlockSource>) {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        let img = ImageFile::open(f.path()).unwrap();
        (f, Arc::new(img))
    }

    #[test]
    fn images_raw_and_verifies() {
        let data: Vec<u8> = (0..8192u32).map(|i| (i % 256) as u8).collect();
        let (_f, src) = make_src(&data);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.img");
        let mapp = dir.path().join("d.map");
        let opts = ImageOptions {
            chunk_size: 1024,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let outcome = image(&src, &out, &mapp, &opts, &mut |_p| {}, &cancel).unwrap();
        assert!(outcome.complete);
        assert!(!outcome.had_bad);
        // Image bytes equal source.
        assert_eq!(std::fs::read(&out).unwrap(), data);
        // Verify passes.
        let rep = verify_image(&out, &mapp).unwrap();
        assert!(rep.whole_ok);
        assert_eq!(rep.chunk_mismatches, 0);
    }

    #[test]
    fn images_zstd_and_verifies() {
        let data: Vec<u8> = (0..8192u32).map(|i| (i * 7 % 256) as u8).collect();
        let (_f, src) = make_src(&data);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.zst");
        let mapp = dir.path().join("d.map");
        let opts = ImageOptions {
            chunk_size: 1024,
            compression: Compression::Zstd,
            sparse: false,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let outcome = image(&src, &out, &mapp, &opts, &mut |_p| {}, &cancel).unwrap();
        assert!(outcome.complete);
        let rep = verify_image(&out, &mapp).unwrap();
        assert!(rep.whole_ok, "zstd whole hash should verify");
        assert_eq!(rep.chunk_mismatches, 0);
    }

    #[test]
    fn records_bad_sectors() {
        let data = vec![0x55u8; 4096];
        let (_f, base) = make_src(&data);
        // Inject a bad sector at LBA 3.
        let faulty: Arc<dyn BlockSource> = Arc::new(FaultInjector::new(base, vec![3u64]));
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("f.img");
        let mapp = dir.path().join("f.map");
        let opts = ImageOptions {
            chunk_size: 4096,
            retries: 1,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let outcome = image(&faulty, &out, &mapp, &opts, &mut |_p| {}, &cancel).unwrap();
        assert!(outcome.complete);
        assert!(outcome.had_bad);
        assert_eq!(outcome.bad_sectors, 1);
    }

    #[test]
    fn resume_with_corrupt_map_errors() {
        let data = vec![1u8; 4096];
        let (_f, src) = make_src(&data);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("c.img");
        let mapp = dir.path().join("c.map");
        {
            use std::io::Write;
            let mut f = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
            f.write_all(b"{ not json").unwrap();
            f.persist(&mapp).unwrap();
        }
        let opts = ImageOptions {
            chunk_size: 1024,
            resume: true,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let err = image(&src, &out, &mapp, &opts, &mut |_p| {}, &cancel).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(!out.exists(), "a failed resume must not touch the image");
        // A map for a different source is rejected the same way.
        let other = ImageMap::new(
            "other".into(),
            4096,
            512,
            1024,
            HashAlgo::Blake3,
            Compression::None,
        );
        other.save(&mapp).unwrap();
        let err = image(&src, &out, &mapp, &opts, &mut |_p| {}, &cancel).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn resume_honors_map_chunk_size() {
        let data: Vec<u8> = (0..8192u32).map(|i| (i * 3 % 256) as u8).collect();
        let (_f, src) = make_src(&data);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("r.img");
        let mapp = dir.path().join("r.map");
        // First run: 1 KiB chunks, cancelled after the first chunk.
        let opts = ImageOptions {
            chunk_size: 1024,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let outcome = image(
            &src,
            &out,
            &mapp,
            &opts,
            &mut |_p| cancel.store(true, Ordering::Relaxed),
            &cancel,
        )
        .unwrap();
        assert!(!outcome.complete);
        let partial = ImageMap::load(&mapp).unwrap();
        assert!(partial.is_chunk_done(0));
        assert!(!partial.is_chunk_done(1));
        // Resume asking for a different chunk size / hash: the map wins.
        let opts = ImageOptions {
            chunk_size: 4096,
            hash_algo: HashAlgo::Sha256,
            resume: true,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let outcome = image(&src, &out, &mapp, &opts, &mut |_p| {}, &cancel).unwrap();
        assert!(outcome.complete);
        let done = ImageMap::load(&mapp).unwrap();
        assert_eq!(done.chunk_size, 1024);
        assert_eq!(done.hash_algo, HashAlgo::Blake3);
        assert_eq!(done.num_chunks(), 8);
        assert_eq!(std::fs::read(&out).unwrap(), data);
        let rep = verify_image(&out, &mapp).unwrap();
        assert!(rep.whole_ok);
        assert_eq!(rep.chunk_mismatches, 0);
    }
}
