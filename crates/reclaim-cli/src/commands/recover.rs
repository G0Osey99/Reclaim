//! `reclaim recover` — copy selected results out to a destination
//! (docs/plan/07 §2). Writes `manifest.json`; refuses a same-whole-disk
//! destination (exit 5). This module is an allow-listed write path.

use crate::commands::common::{dest_on_same_disk, resolve_session};
use crate::exit::{CmdError, CmdResult, Exit};
use crate::source::Resolved;
use reclaim_block::imaging::hash::Digest;
use reclaim_block::imaging::HashAlgo;
use reclaim_carve::reader::{Reader, SourceReader};
use reclaim_session::{CarvedRecord, QueryFilter, Store};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Name-collision policy.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Collision {
    /// Append `-1`, `-2`, … to avoid clobbering.
    Rename,
    /// Skip if the destination exists.
    Skip,
    /// Overwrite.
    Overwrite,
}

impl Collision {
    /// Parse `--collision`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rename" => Some(Collision::Rename),
            "skip" => Some(Collision::Skip),
            "overwrite" => Some(Collision::Overwrite),
            _ => None,
        }
    }
}

/// Flags for `recover`.
pub struct RecoverArgs<'a> {
    pub session_positional: Option<&'a Path>,
    pub session_global: Option<&'a Path>,
    pub dest: PathBuf,
    pub ids: Option<Vec<String>>,
    pub all: bool,
    pub filter: QueryFilter,
    pub preserve_paths: bool,
    pub flat: bool,
    pub collision: Collision,
    pub verify: bool,
    pub allow_same_device: bool,
    pub quiet: bool,
}

const CHUNK: usize = 4 * 1024 * 1024;

/// Run `recover`.
pub fn run(args: &RecoverArgs) -> CmdResult {
    let dir = resolve_session(args.session_positional, args.session_global)?;
    let store = Store::open(&dir.join("session.sqlite"))
        .map_err(|e| CmdError::not_found(format!("open session {}: {e}", dir.display())))?;
    let source_id = read_source_id(&dir)?;
    // A `mounted:` session copies live files from the walked directory by path,
    // rather than reading device extents (docs/plan/06 §6, §8).
    if let Some(root) = source_id.strip_prefix("mounted:") {
        return recover_mounted(Path::new(root), &store, args, &dir, &source_id);
    }
    let resolved = Resolved::from_source_id(&source_id).ok_or_else(|| {
        CmdError::not_found(format!("cannot reconstruct source from id {source_id:?}"))
    })?;

    // Same-whole-disk refusal (docs/plan/07 §5).
    if !args.allow_same_device && dest_on_same_disk(&resolved, &args.dest) {
        return Err(CmdError::new(
            Exit::Refused,
            format!(
                "destination {} is on the same disk as the source — refusing (data-loss risk). \
                 Pass --allow-same-device-i-accept-data-loss to override.",
                args.dest.display()
            ),
        ));
    }

    let src = resolved.open()?;
    let reader = SourceReader::new(Arc::clone(&src));

    // Select records.
    let mut filter = args.filter.clone();
    if let Some(ids) = &args.ids {
        filter.ids = Some(ids.clone());
    } else if !args.all && filter.ids.is_none() && is_empty_filter(&filter) {
        return Err(CmdError::new(
            Exit::Usage,
            "recover needs --all, --ids, or filters to select results",
        ));
    }
    let records = store
        .query(&filter)
        .map_err(|e| CmdError::internal(e.to_string()))?;

    std::fs::create_dir_all(&args.dest)
        .map_err(|e| CmdError::internal(format!("create {}: {e}", args.dest.display())))?;

    let mut manifest: Vec<serde_json::Value> = Vec::with_capacity(records.len());
    let mut partial = 0usize;
    let mut recovered = 0usize;

    for rec in &records {
        let rel = out_rel_path(rec, args.preserve_paths, args.flat);
        let out_path = args.dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CmdError::internal(format!("create dir: {e}")))?;
        }
        let final_path = match resolve_collision(&out_path, args.collision) {
            Some(p) => p,
            None => continue, // skip
        };

        let hash = write_extent(&reader, rec, &final_path)?;
        recovered += 1;
        if rec.validity != "full" {
            partial += 1;
        }
        let rel_final = final_path
            .strip_prefix(&args.dest)
            .unwrap_or(&final_path)
            .to_string_lossy()
            .to_string();
        manifest.push(serde_json::json!({
            "id": rec.id,
            "path": rel_final,
            "source_offset": rec.offset,
            "len": rec.len,
            "format": rec.format,
            "validity": rec.validity,
            "blake3": hash,
            "verified": args.verify,
        }));
        if !args.quiet {
            eprintln!(
                "recovered {rel_final} ({} bytes, {})",
                rec.len, rec.validity
            );
        }
    }

    let manifest_doc = serde_json::json!({
        "source_id": source_id,
        "session": dir.to_string_lossy(),
        "count": manifest.len(),
        "files": manifest,
    });
    let mpath = args.dest.join("manifest.json");
    std::fs::write(
        &mpath,
        serde_json::to_string_pretty(&manifest_doc).unwrap_or_else(|_| "{}".to_string()),
    )
    .map_err(|e| CmdError::internal(format!("write manifest: {e}")))?;

    if !args.quiet {
        eprintln!(
            "recovered {recovered} file(s) to {} ({partial} partial/suspect). manifest: {}",
            args.dest.display(),
            mpath.display()
        );
    }
    if partial > 0 {
        Ok(Exit::Warnings)
    } else {
        Ok(Exit::Success)
    }
}

/// Recover from a `mounted:` session by copying live/Trash files by path.
fn recover_mounted(
    root: &Path,
    store: &Store,
    args: &RecoverArgs,
    dir: &Path,
    source_id: &str,
) -> CmdResult {
    use std::os::unix::fs::MetadataExt;
    // Refuse a destination on the same filesystem device as the source volume
    // unless explicitly overridden (data-loss safety, docs/plan/07 §5).
    if !args.allow_same_device {
        let root_dev = std::fs::metadata(root).ok().map(|m| m.dev());
        let mut anc = args.dest.as_path();
        let dest_dev = loop {
            if let Ok(m) = std::fs::metadata(anc) {
                break Some(m.dev());
            }
            match anc.parent() {
                Some(p) => anc = p,
                None => break None,
            }
        };
        if let (Some(a), Some(b)) = (root_dev, dest_dev) {
            if a == b {
                return Err(CmdError::new(
                    Exit::Refused,
                    format!(
                        "destination {} is on the same volume as the mounted source — refusing. \
                         Pass --allow-same-device-i-accept-data-loss to override.",
                        args.dest.display()
                    ),
                ));
            }
        }
    }

    let mut filter = args.filter.clone();
    if let Some(ids) = &args.ids {
        filter.ids = Some(ids.clone());
    } else if !args.all && filter.ids.is_none() && is_empty_filter(&filter) {
        return Err(CmdError::new(
            Exit::Usage,
            "recover needs --all, --ids, or filters to select results",
        ));
    }
    let records = store
        .query(&filter)
        .map_err(|e| CmdError::internal(e.to_string()))?;
    std::fs::create_dir_all(&args.dest)
        .map_err(|e| CmdError::internal(format!("create {}: {e}", args.dest.display())))?;

    let mut manifest: Vec<serde_json::Value> = Vec::with_capacity(records.len());
    let mut recovered = 0usize;
    let mut missing = 0usize;
    for rec in &records {
        let src_file = root.join(rec.synth_path());
        let rel = out_rel_path(rec, args.preserve_paths, args.flat);
        let out_path = args.dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CmdError::internal(format!("create dir: {e}")))?;
        }
        let final_path = match resolve_collision(&out_path, args.collision) {
            Some(p) => p,
            None => continue,
        };
        let bytes = match std::fs::read(&src_file) {
            Ok(b) => b,
            Err(_) => {
                missing += 1;
                continue; // the live file is gone (already emptied from Trash)
            }
        };
        let mut digest = Digest::new(HashAlgo::Blake3);
        digest.update(&bytes);
        std::fs::write(&final_path, &bytes)
            .map_err(|e| CmdError::internal(format!("write {}: {e}", final_path.display())))?;
        recovered += 1;
        let rel_final = final_path
            .strip_prefix(&args.dest)
            .unwrap_or(&final_path)
            .to_string_lossy()
            .to_string();
        manifest.push(serde_json::json!({
            "id": rec.id,
            "path": rel_final,
            "source_path": src_file.to_string_lossy(),
            "len": bytes.len(),
            "state": rec.state,
            "blake3": digest.finalize_hex(),
            "verified": args.verify,
        }));
        if !args.quiet {
            eprintln!("recovered {rel_final} ({} bytes)", bytes.len());
        }
    }
    let manifest_doc = serde_json::json!({
        "source_id": source_id,
        "session": dir.to_string_lossy(),
        "count": manifest.len(),
        "files": manifest,
    });
    std::fs::write(
        args.dest.join("manifest.json"),
        serde_json::to_string_pretty(&manifest_doc).unwrap_or_else(|_| "{}".to_string()),
    )
    .map_err(|e| CmdError::internal(format!("write manifest: {e}")))?;
    if !args.quiet {
        eprintln!(
            "recovered {recovered} file(s) to {} ({missing} no longer present).",
            args.dest.display()
        );
    }
    Ok(Exit::Success)
}

/// Read the source id from `source.json`.
fn read_source_id(dir: &Path) -> Result<String, CmdError> {
    let text = std::fs::read_to_string(dir.join("source.json"))
        .map_err(|e| CmdError::not_found(format!("read source.json: {e}")))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| CmdError::internal(e.to_string()))?;
    v.get("source_id")
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .ok_or_else(|| CmdError::internal("source.json missing source_id"))
}

fn is_empty_filter(f: &QueryFilter) -> bool {
    f.family.is_none()
        && f.exts.is_empty()
        && f.min_size.is_none()
        && f.after.is_none()
        && f.min_score.is_none()
        && f.engine.is_none()
        && f.path_glob.is_none()
        && !f.full_only
}

fn out_rel_path(rec: &CarvedRecord, preserve: bool, flat: bool) -> PathBuf {
    let synth = rec.synth_path();
    if flat || !preserve {
        // Flat: just the file name.
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

/// Write a result's source extent to `out_path`, returning the BLAKE3 hex.
fn write_extent(
    reader: &SourceReader,
    rec: &CarvedRecord,
    out_path: &Path,
) -> Result<String, CmdError> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(out_path)
        .map_err(|e| CmdError::internal(format!("open {}: {e}", out_path.display())))?;
    let mut digest = Digest::new(HashAlgo::Blake3);
    // Named filesystem entries may be fragmented (multiple extents); carved
    // results are a single contiguous (offset,len). `extents()` returns either.
    for (ext_off, ext_len) in rec.extents() {
        let mut off = ext_off;
        let mut remaining = ext_len;
        while remaining > 0 {
            let n = remaining.min(CHUNK as u64) as usize;
            let buf = reader.read(off, n);
            f.write_all(&buf)
                .map_err(|e| CmdError::internal(format!("write {}: {e}", out_path.display())))?;
            digest.update(&buf);
            off += n as u64;
            remaining -= n as u64;
        }
    }
    f.flush().map_err(|e| CmdError::internal(e.to_string()))?;
    Ok(digest.finalize_hex())
}
