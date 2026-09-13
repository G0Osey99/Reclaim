//! Recovery output: copy selected results' extents to a destination with a
//! collision policy, optional **verify-after-copy**, and a JSON manifest
//! (docs/plan/02 FR-RES-3/4). Also the small temp-extraction helper the GUI
//! uses to hand a single result's bytes to PDFKit / AVFoundation for a preview
//! (docs/plan/08 §2.3) — that extraction lands in the *session* directory,
//! never on the source.
//!
//! This is the library home for "recovered files" (build guide Part 3.2:
//! "Writes exist in exactly two places: …dest.rs and reclaim-session"), so both
//! the GUI FFI and any future consolidation of the CLI driver share one copy of
//! the extent-copy + verify logic. The same-disk destination refusal and the
//! free-space check need platform/source context and live in the caller
//! (`reclaim-ffi`), which calls this only after those guards pass.

use crate::store::CarvedRecord;
use crate::SessionError;
use reclaim_block::imaging::hash::Digest;
use reclaim_block::imaging::HashAlgo;
use reclaim_carve::reader::{Reader, SourceReader};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const CHUNK: usize = 4 * 1024 * 1024;

/// Name-collision policy (mirror of the CLI's `Collision`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Collision {
    /// Append `-1`, `-2`, … to avoid clobbering.
    Rename,
    /// Skip if the destination exists.
    Skip,
    /// Overwrite.
    Overwrite,
}

/// Recovery options.
#[derive(Clone, Debug)]
pub struct RecoverOpts {
    /// Recreate the source folder hierarchy under `dest`.
    pub preserve_paths: bool,
    /// Flatten to just the file name (overrides `preserve_paths`).
    pub flat: bool,
    /// Name-collision policy.
    pub collision: Collision,
    /// Re-read each written file and confirm its hash matches the copy pass.
    pub verify: bool,
}

impl Default for RecoverOpts {
    fn default() -> Self {
        RecoverOpts {
            preserve_paths: true,
            flat: false,
            collision: Collision::Rename,
            verify: true,
        }
    }
}

/// Per-file recovery outcome (also serialized into the manifest).
#[derive(Clone, Debug)]
pub struct RecoveredFile {
    /// Result id.
    pub id: String,
    /// Destination-relative path written.
    pub path: String,
    /// Bytes written.
    pub len: u64,
    /// `full` / `truncated` / `suspect`.
    pub validity: String,
    /// BLAKE3 of the written bytes.
    pub blake3: String,
    /// `true` when `verify` ran and the re-read hash matched; `false` when it
    /// mismatched; `None` when verification was not requested.
    pub verified: Option<bool>,
}

/// Summary of a recover run.
#[derive(Clone, Debug, Default)]
pub struct RecoverSummary {
    /// Files written.
    pub recovered: u64,
    /// Files skipped by the collision policy.
    pub skipped: u64,
    /// Files whose validity was not `full` (truncated/suspect).
    pub partial: u64,
    /// Files that failed verify-after-copy.
    pub verify_failed: u64,
    /// Total bytes written.
    pub bytes: u64,
    /// Absolute path of the written `manifest.json`.
    pub manifest_path: String,
    /// Per-file details.
    pub files: Vec<RecoveredFile>,
}

/// Copy each record's extents to `dest`, honoring the collision policy, and
/// (when `opts.verify`) re-read each file to confirm its hash. Writes
/// `dest/manifest.json`. The caller must have already opened `reader` over the
/// session's source and cleared the same-disk / free-space guards.
pub fn recover_records(
    dest: &Path,
    source_id: &str,
    records: &[CarvedRecord],
    reader: &SourceReader,
    opts: &RecoverOpts,
    progress: &mut dyn FnMut(u64, u64, &RecoveredFile),
) -> Result<RecoverSummary, SessionError> {
    std::fs::create_dir_all(dest)?;
    let total = records.len() as u64;
    let mut summary = RecoverSummary::default();
    for rec in records {
        let rel = out_rel_path(rec, opts.preserve_paths, opts.flat);
        let out_path = dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let final_path = match resolve_collision(&out_path, opts.collision) {
            Some(p) => p,
            None => {
                summary.skipped += 1;
                continue;
            }
        };
        let hash = write_extent(reader, rec, &final_path)?;
        summary.recovered += 1;
        summary.bytes += rec.len;
        if rec.validity != "full" {
            summary.partial += 1;
        }
        let verified = if opts.verify {
            let ok = verify_file(&final_path, &hash)?;
            if !ok {
                summary.verify_failed += 1;
            }
            Some(ok)
        } else {
            None
        };
        let rel_final = final_path
            .strip_prefix(dest)
            .unwrap_or(&final_path)
            .to_string_lossy()
            .to_string();
        let file = RecoveredFile {
            id: rec.id.clone(),
            path: rel_final,
            len: rec.len,
            validity: rec.validity.clone(),
            blake3: hash,
            verified,
        };
        progress(summary.recovered, total, &file);
        summary.files.push(file);
    }

    let manifest = serde_json::json!({
        "source_id": source_id,
        "dest": dest.to_string_lossy(),
        "count": summary.recovered,
        "partial": summary.partial,
        "verify_failed": summary.verify_failed,
        "files": summary.files.iter().map(|f| serde_json::json!({
            "id": f.id,
            "path": f.path,
            "len": f.len,
            "validity": f.validity,
            "blake3": f.blake3,
            "verified": f.verified,
        })).collect::<Vec<_>>(),
    });
    let mpath = dest.join("manifest.json");
    std::fs::write(
        &mpath,
        serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string()),
    )?;
    summary.manifest_path = mpath.to_string_lossy().to_string();
    Ok(summary)
}

/// Extract a single result's bytes to a temp file under the session directory so
/// a system framework (PDFKit / AVFoundation) can open it for a preview. The
/// file goes in `session_dir/preview-tmp/`, never on the source. Returns the
/// written path. `max_bytes` caps very large results.
pub fn extract_to_temp(
    session_dir: &Path,
    rec: &CarvedRecord,
    reader: &SourceReader,
    max_bytes: u64,
) -> Result<PathBuf, SessionError> {
    let tmp = session_dir.join("preview-tmp");
    std::fs::create_dir_all(&tmp)?;
    let ext = if rec.ext.is_empty() { "bin" } else { &rec.ext };
    let out = tmp.join(format!("{}.{ext}", rec.id));
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&out)?;
    let mut written = 0u64;
    'extents: for (ext_off, ext_len) in rec.extents() {
        let mut off = ext_off;
        let mut remaining = ext_len.min(max_bytes.saturating_sub(written));
        while remaining > 0 {
            let n = remaining.min(CHUNK as u64) as usize;
            let buf = reader.read(off, n);
            f.write_all(&buf)?;
            off += n as u64;
            remaining -= n as u64;
            written += n as u64;
            if written >= max_bytes {
                break 'extents;
            }
        }
    }
    f.flush()?;
    Ok(out)
}

fn out_rel_path(rec: &CarvedRecord, preserve: bool, flat: bool) -> PathBuf {
    let synth = rec.synth_path();
    if flat || !preserve {
        let name = synth.rsplit('/').next().unwrap_or(&synth);
        PathBuf::from(name)
    } else {
        PathBuf::from(synth)
    }
}

fn resolve_collision(path: &Path, policy: Collision) -> Option<PathBuf> {
    if !path.exists() {
        return Some(path.to_path_buf());
    }
    match policy {
        Collision::Overwrite => Some(path.to_path_buf()),
        Collision::Skip => None,
        Collision::Rename => {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let ext = path.extension().map(|s| s.to_string_lossy().to_string());
            let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
            for n in 1..100_000u32 {
                let name = match &ext {
                    Some(e) => format!("{stem}-{n}.{e}"),
                    None => format!("{stem}-{n}"),
                };
                let cand = parent.join(name);
                if !cand.exists() {
                    return Some(cand);
                }
            }
            None
        }
    }
}

/// Write a result's source extents to `out_path`, returning the BLAKE3 hex of
/// the bytes written.
fn write_extent(
    reader: &SourceReader,
    rec: &CarvedRecord,
    out_path: &Path,
) -> Result<String, SessionError> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(out_path)?;
    let mut digest = Digest::new(HashAlgo::Blake3);
    for (ext_off, ext_len) in rec.extents() {
        let mut off = ext_off;
        let mut remaining = ext_len;
        while remaining > 0 {
            let n = remaining.min(CHUNK as u64) as usize;
            let buf = reader.read(off, n);
            f.write_all(&buf)?;
            digest.update(&buf);
            off += n as u64;
            remaining -= n as u64;
        }
    }
    f.flush()?;
    Ok(digest.finalize_hex())
}

/// Re-read a written file and confirm its BLAKE3 equals `expected`.
fn verify_file(path: &Path, expected: &str) -> Result<bool, SessionError> {
    let mut f = std::fs::File::open(path)?;
    let mut digest = Digest::new(HashAlgo::Blake3);
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if let Some(chunk) = buf.get(..n) {
            digest.update(chunk);
        }
    }
    Ok(digest.finalize_hex() == expected)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use reclaim_block::MemorySource;
    use std::sync::Arc;

    fn record(id: &str, off: u64, len: u64, ext: &str) -> CarvedRecord {
        CarvedRecord {
            id: id.into(),
            source_id: "src".into(),
            engine: "carve".into(),
            offset: off,
            len,
            format: format!("image.{ext}"),
            family: "image".into(),
            ext: ext.into(),
            validity: "full".into(),
            score: 90,
            block_aligned: true,
            name: None,
            date: None,
            model: None,
            thumb_offset: None,
            thumb_len: None,
            path: None,
            state: None,
            kind: "file".into(),
            extents_json: None,
            merged: false,
        }
    }

    #[test]
    fn recover_and_verify() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let src: Arc<dyn reclaim_block::BlockSource> = Arc::new(MemorySource::new(data.clone()));
        let reader = SourceReader::new(src);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        let recs = vec![record("aaa", 100, 500, "jpg")];
        let sum = recover_records(
            &dest,
            "src",
            &recs,
            &reader,
            &RecoverOpts::default(),
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(sum.recovered, 1);
        assert_eq!(sum.verify_failed, 0);
        assert_eq!(sum.files[0].verified, Some(true));
        // The written bytes equal the source extent.
        let written = std::fs::read(dest.join(&sum.files[0].path)).unwrap();
        assert_eq!(written, data[100..600].to_vec());
        assert!(dest.join("manifest.json").exists());
    }

    #[test]
    fn collision_skip_and_rename() {
        let src: Arc<dyn reclaim_block::BlockSource> = Arc::new(MemorySource::new(vec![7u8; 4096]));
        let reader = SourceReader::new(src);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        let recs = vec![record("a", 0, 16, "bin"), record("b", 0, 16, "bin")];
        // Flat → both want the same name; rename keeps both.
        let opts = RecoverOpts {
            preserve_paths: false,
            flat: true,
            collision: Collision::Rename,
            verify: false,
        };
        let sum = recover_records(&dest, "src", &recs, &reader, &opts, &mut |_, _, _| {}).unwrap();
        assert_eq!(sum.recovered, 2);
        let names: Vec<_> = std::fs::read_dir(&dest)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .filter(|n| n != "manifest.json")
            .collect();
        assert_eq!(names.len(), 2); // one renamed
    }

    #[test]
    fn extract_temp_stays_in_session() {
        let data: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let src: Arc<dyn reclaim_block::BlockSource> = Arc::new(MemorySource::new(data));
        let reader = SourceReader::new(src);
        let sdir = tempfile::tempdir().unwrap();
        let rec = record("xyz", 10, 200, "pdf");
        let p = extract_to_temp(sdir.path(), &rec, &reader, 1 << 20).unwrap();
        assert!(p.starts_with(sdir.path()));
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 200);
    }
}
