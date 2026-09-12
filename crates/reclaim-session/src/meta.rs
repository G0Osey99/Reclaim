//! Metadata-engine orchestration (build guide Phase-2 prompt D).
//!
//! `run_quick` parses the partition scheme ([`reclaim_part`]), builds an
//! `OffsetView` per volume, probes the registered filesystem engines
//! (exFAT/FAT/NTFS), walks the best match, and stores each named entry with a
//! recoverability score. Extents emitted by an engine are volume-relative; the
//! sink shifts them to absolute source offsets so they share one coordinate
//! space with carved results for the merge (docs/plan/03 §2.3 / §5 step 6).

use crate::events::Event;
use crate::store::EntryRow;
use crate::{Session, SessionError};
use reclaim_block::{BlockSource, OffsetView};
use reclaim_fs_core::{
    score::recoverability, Bitmap, Entry, EntryKind, EntrySink, OpenFn, Probe, ProbeFn, WalkOpts,
};
use std::sync::Arc;

/// A registered filesystem engine.
struct EngineDef {
    name: &'static str,
    probe: ProbeFn,
    open: OpenFn,
}

/// The engine registry (order is priority on a probe tie). Phase 3 adds the
/// APFS and HFS+ engines alongside the Phase-2 exFAT/FAT/NTFS ones.
fn engines() -> [EngineDef; 7] {
    [
        EngineDef {
            name: "apfs",
            probe: fs_apfs::probe,
            open: fs_apfs::open_boxed,
        },
        EngineDef {
            name: "hfs+",
            probe: fs_hfs::probe,
            open: fs_hfs::open_boxed,
        },
        EngineDef {
            name: "ntfs",
            probe: fs_ntfs::probe,
            open: fs_ntfs::open_boxed,
        },
        EngineDef {
            name: "exfat",
            probe: fs_exfat::probe,
            open: fs_exfat::open_boxed,
        },
        EngineDef {
            name: "fat",
            probe: fs_fat::probe,
            open: fs_fat::open_boxed,
        },
        EngineDef {
            name: "ext",
            probe: fs_ext::probe,
            open: fs_ext::open_boxed,
        },
        EngineDef {
            name: "iso9660",
            probe: fs_iso::probe,
            open: fs_iso::open_boxed,
        },
    ]
}

/// An allocation bitmap anchored at an absolute source offset (the volume base).
#[derive(Clone, Debug)]
pub struct AbsBitmap {
    /// Absolute byte offset of the volume the bitmap describes.
    pub base: u64,
    /// The volume-relative bitmap.
    pub bitmap: Bitmap,
}

impl AbsBitmap {
    /// Allocation state at an absolute source offset (offsets outside the
    /// volume are treated as allocated/unknown).
    #[must_use]
    pub fn is_allocated(&self, abs: u64) -> bool {
        if abs < self.base {
            return true;
        }
        self.bitmap.is_offset_allocated(abs - self.base)
    }
}

/// Report from a quick (metadata) scan.
#[derive(Clone, Debug, Default)]
pub struct MetaReport {
    /// Volumes a metadata engine handled.
    pub volumes_handled: usize,
    /// Named entries stored.
    pub entries: u64,
    /// Deleted/orphaned/historical entries among them.
    pub deleted: u64,
    /// Partition scheme label.
    pub scheme: String,
    /// Allocation bitmaps (absolute), for the carver's `--unallocated-only` and
    /// allocated/unallocated labeling.
    pub bitmaps: Vec<AbsBitmap>,
}

impl Session {
    /// Run the metadata engines over `src` (quick pass). Returns a summary and
    /// emits `Found`/`Warning`/`PassComplete` events.
    pub fn run_quick(
        &mut self,
        src: &Arc<dyn BlockSource>,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<MetaReport, SessionError> {
        use std::io::Write as _;
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.log_path())?;
        let mut emit = |ev: &Event, cb: &mut dyn FnMut(&Event)| {
            let _ = writeln!(log, "{}", ev.to_ndjson());
            cb(ev);
        };

        let map = reclaim_part::scan(src);
        let mut report = MetaReport {
            scheme: map.scheme.label().to_string(),
            ..Default::default()
        };
        emit(
            &Event::Warning {
                msg: format!(
                    "partition scheme: {} ({} partition(s), confidence {:.2})",
                    map.scheme.label(),
                    map.entries.len(),
                    map.confidence
                ),
            },
            on_event,
        );

        let source_id = self.source.source_id.clone();
        for (start, len) in map.volume_windows(src.len()) {
            if len == 0 {
                continue;
            }
            let view: Arc<dyn BlockSource> = match OffsetView::new(Arc::clone(src), start, len) {
                Ok(v) => Arc::new(v),
                Err(_) => continue,
            };
            // Probe every engine; take the best above threshold.
            let registry = engines();
            let mut best: Option<(f32, &EngineDef, Probe)> = None;
            for e in &registry {
                if let Some(p) = (e.probe)(&view) {
                    if p.confidence >= 0.5
                        && best.as_ref().map(|b| p.confidence > b.0).unwrap_or(true)
                    {
                        best = Some((p.confidence, e, p));
                    }
                }
            }
            let Some((_, engine_def, probe)) = best else {
                continue;
            };
            let fs = match (engine_def.open)(Arc::clone(&view), probe.clone()) {
                Ok(f) => f,
                Err(err) => {
                    emit(
                        &Event::Warning {
                            msg: format!("{} open failed at +{start}: {err}", engine_def.name),
                        },
                        on_event,
                    );
                    continue;
                }
            };
            emit(
                &Event::Warning {
                    msg: format!(
                        "{} volume at +{start} ({} bytes), cluster {}",
                        engine_def.name,
                        len,
                        fs.block_size()
                    ),
                },
                on_event,
            );

            let bitmap = fs.allocation_bitmap();
            if let Some(bm) = bitmap.clone() {
                report.bitmaps.push(AbsBitmap {
                    base: start,
                    bitmap: bm,
                });
            }
            let mut sink = CollectSink {
                rows: Vec::new(),
                base: start,
                bitmap,
            };
            let stats = fs
                .walk(&mut sink, &WalkOpts::default())
                .map_err(|e| SessionError::Other(format!("{} walk: {e}", engine_def.name)))?;

            // Store and emit.
            let engine_name = fs.kind().to_string();
            for chunk in sink.rows.chunks(1024) {
                let ids = self.store.insert_entries(&source_id, &engine_name, chunk)?;
                for (row, id) in chunk.iter().zip(ids.iter()) {
                    let ev = Event::Found {
                        id: id.clone(),
                        engine: engine_name.clone(),
                        path: row.path.clone(),
                        size: row.len,
                        score: row.score,
                    };
                    let _ = self.store.put_event("found", &ev.to_ndjson());
                    emit(&ev, on_event);
                }
            }
            report.volumes_handled += 1;
            report.entries += sink.rows.len() as u64;
            report.deleted += stats.deleted;
            emit(&Event::PassComplete { pass: engine_name }, on_event);
        }
        Ok(report)
    }
}

impl Session {
    /// Walk a **mounted** volume/directory read-only via POSIX and store each
    /// regular file as an entry (docs/plan/06 §6, §8). Files under a Trash
    /// directory (`.Trash`, `.Trashes`, `Trash`) are stored `deleted` — the
    /// "did you check the Trash?" recovery step — everything else `live`. No
    /// block device is opened; `recover` copies these by path.
    pub fn run_mounted(
        &mut self,
        root: &std::path::Path,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<MetaReport, SessionError> {
        let source_id = self.source.source_id.clone();
        let mut report = MetaReport {
            scheme: "mounted".to_string(),
            ..Default::default()
        };
        let mut rows: Vec<EntryRow> = Vec::new();
        let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
        let mut budget = 5_000_000usize;
        while let Some(dir) = stack.pop() {
            if budget == 0 {
                break;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for ent in entries.flatten() {
                let path = ent.path();
                let ft = match ent.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if ft.is_symlink() {
                    continue; // never follow symlinks (read-only, loop-safe)
                }
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !ft.is_file() {
                    continue;
                }
                budget = budget.saturating_sub(1);
                let rel = path.strip_prefix(root).unwrap_or(&path);
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                let in_trash = rel.components().any(|c| {
                    matches!(
                        c.as_os_str().to_string_lossy().as_ref(),
                        ".Trash" | ".Trashes" | "Trash"
                    )
                });
                let meta = ent.metadata().ok();
                let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let name = ent.file_name().to_string_lossy().into_owned();
                let ext = rel_str
                    .rsplit('.')
                    .next()
                    .filter(|e| *e != rel_str && !e.is_empty())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let family = family_for_ext(&ext);
                let format = if ext.is_empty() {
                    "file".to_string()
                } else {
                    format!("{family}.{ext}")
                };
                rows.push(EntryRow {
                    path: rel_str.clone(),
                    name,
                    kind: "file",
                    state: if in_trash { "deleted" } else { "live" }.to_string(),
                    family,
                    ext,
                    format,
                    offset: pseudo_offset(&rel_str),
                    len,
                    extents: Vec::new(),
                    score: if in_trash { 90 } else { 99 },
                    date: None,
                    contiguous_assumed: false,
                });
                if in_trash {
                    report.deleted += 1;
                }
            }
        }
        for chunk in rows.chunks(1024) {
            let ids = self.store.insert_entries(&source_id, "mounted", chunk)?;
            for (row, id) in chunk.iter().zip(ids.iter()) {
                let ev = Event::Found {
                    id: id.clone(),
                    engine: "mounted".to_string(),
                    path: row.path.clone(),
                    size: row.len,
                    score: row.score,
                };
                let _ = self.store.put_event("found", &ev.to_ndjson());
                on_event(&ev);
            }
        }
        report.volumes_handled = 1;
        report.entries = rows.len() as u64;
        Ok(report)
    }

    /// Label carved results `allocated`/`unallocated` against the FS bitmaps and,
    /// when `unallocated_only`, hide (merge out) the allocated ones — a live
    /// file's content is not a deletion. Returns `(allocated, unallocated)`.
    pub fn label_carved(
        &mut self,
        bitmaps: &[AbsBitmap],
        unallocated_only: bool,
    ) -> Result<(u64, u64), SessionError> {
        if bitmaps.is_empty() {
            return Ok((0, 0));
        }
        let rows = self.store.carve_offsets()?;
        let mut allocated = 0u64;
        let mut unallocated = 0u64;
        let mut updates: Vec<(String, &'static str, bool)> = Vec::new();
        for (id, offset) in rows {
            let is_alloc = bitmaps.iter().any(|b| b.is_allocated(offset));
            let state = if is_alloc { "allocated" } else { "unallocated" };
            if is_alloc {
                allocated += 1;
            } else {
                unallocated += 1;
            }
            let hide = unallocated_only && is_alloc;
            updates.push((id, state, hide));
        }
        self.store.apply_carve_labels(&updates)?;
        Ok((allocated, unallocated))
    }
}

/// A sink that converts each engine [`Entry`] into a stored [`EntryRow`],
/// scoring it against the allocation bitmap (volume-relative) and shifting its
/// extents to absolute source offsets.
struct CollectSink {
    rows: Vec<EntryRow>,
    base: u64,
    bitmap: Option<Bitmap>,
}

impl EntrySink for CollectSink {
    fn emit(&mut self, entry: Entry) {
        // Store files only; directory structure is reconstructed from paths.
        if entry.kind == EntryKind::Dir {
            return;
        }
        let score = recoverability(&entry, self.bitmap.as_ref());
        // Shift extents to absolute.
        let extents: Vec<(u64, u64)> = entry
            .extents
            .iter()
            .map(|e| (self.base.saturating_add(e.offset), e.len))
            .collect();
        let offset = extents
            .first()
            .map(|(o, _)| *o)
            .unwrap_or_else(|| pseudo_offset(entry.path.as_deref().unwrap_or(&entry.name)));
        let path = entry.path.clone().unwrap_or_else(|| entry.name.clone());
        let ext = path
            .rsplit('.')
            .next()
            .filter(|e| *e != path && !e.is_empty())
            .unwrap_or("")
            .to_ascii_lowercase();
        let family = family_for_ext(&ext);
        let format = if ext.is_empty() {
            "file".to_string()
        } else {
            format!("{family}.{ext}")
        };
        self.rows.push(EntryRow {
            path,
            name: entry.name.clone(),
            kind: "file",
            state: entry.state.label().to_string(),
            family,
            ext,
            format,
            offset,
            len: entry.size,
            extents,
            score,
            date: entry.modified.clone().or(entry.created.clone()),
            contiguous_assumed: entry.contiguous_assumed,
        });
    }
}

/// A stable pseudo-offset for content-less entries so their deterministic id
/// does not collide (FNV-1a of the path, high bit set to avoid data ranges).
fn pseudo_offset(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    0x8000_0000_0000_0000 | (h & 0x7FFF_FFFF_FFFF_FFFF)
}

/// Map an extension to a carve family so `results --family` works uniformly.
fn family_for_ext(ext: &str) -> String {
    if ext.is_empty() {
        return "file".to_string();
    }
    // Prefer the catalog's own family for this extension.
    for s in reclaim_sigs::all() {
        if s.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            return s.family.to_string();
        }
    }
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff" | "heic" | "webp" => "image",
        "mp4" | "mov" | "avi" | "mkv" | "m4v" => "video",
        "mp3" | "flac" | "wav" | "aac" | "m4a" => "audio",
        "txt" | "pdf" | "doc" | "docx" | "rtf" => "doc",
        "zip" | "rar" | "7z" | "gz" | "tar" => "archive",
        _ => "file",
    }
    .to_string()
}
