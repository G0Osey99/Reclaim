//! Reclaim NTFS metadata engine (docs/plan/04 §2 NTFS row, §3.3).
//!
//! # Crate-vs-own decision (build guide Phase-2 rule)
//! The permissive `ntfs` crate (ColinFinck, MIT/Apache-2.0) was evaluated. It is
//! a solid reader for **live** NTFS volumes, but Reclaim's value here is the
//! recovery of **deleted** records, orphan `FILE` records after a reformat,
//! `$I30` index slack and `$UsnJrnl:$J` names — none of which that crate
//! exposes (its API walks the live directory tree). Pulling it in would add a
//! dependency while still requiring us to hand-roll the deleted/orphan mining,
//! and it would not share the no-unwrap / read-only discipline of fs-exfat and
//! fs-fat. We therefore **implement our own** parser from the public NTFS
//! documentation (same approach as the Phase-1 validators), and note the
//! evaluation here and in the build log.
//!
//! The engine parses the boot sector, applies MFT record fix-ups, walks every
//! `FILE` record (in-use flag ⇒ live/deleted), reads `$STANDARD_INFORMATION`,
//! `$FILE_NAME` (preferring Win32 names) and `$DATA` (resident + non-resident
//! data runs, noting compressed/sparse), loads `$Bitmap`, scans for orphan
//! `FILE` records, and best-effort mines `$I30` slack and `$UsnJrnl:$J` names.
//!
//! Parses hostile bytes; denies panicking accessors (build guide Part 1.4 rule 3).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

mod date;
pub mod record;
pub mod usn;

use reclaim_block::BlockSource;
use reclaim_fs_core::{
    Bitmap, Entry, EntryKind, EntrySink, EntryState, Extent, FileSystem, FsError, Probe, WalkOpts,
    WalkStats,
};
use record::{MftRecord, ATTR_BITMAP, ATTR_DATA};
use std::collections::BTreeMap;
use std::sync::Arc;

const FILE_MAGIC: &[u8; 4] = b"FILE";
const ROOT_REF: u64 = 5;

/// An opened NTFS volume.
pub struct Ntfs {
    src: Arc<dyn BlockSource>,
    cluster_size: u64,
    mft_offset: u64,
    mft_record_size: u64,
    total_bytes: u64,
    serial: u64,
    /// The $MFT's own data extents (volume-relative), so records can be located
    /// even when the MFT is fragmented.
    mft_extents: Vec<Extent>,
}

impl std::fmt::Debug for Ntfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ntfs")
            .field("cluster_size", &self.cluster_size)
            .field("mft_offset", &self.mft_offset)
            .field("mft_record_size", &self.mft_record_size)
            .finish()
    }
}

/// Probe a volume source for NTFS (docs/plan/04 §2).
#[must_use]
pub fn probe(src: &Arc<dyn BlockSource>) -> Option<Probe> {
    let boot = read(src, 0, 512);
    if boot.get(3..11) != Some(b"NTFS    ") {
        return None;
    }
    let bps = u64::from(le_u16(&boot, 11));
    if !matches!(bps, 512 | 1024 | 2048 | 4096) {
        return None;
    }
    if boot.get(510..512) != Some(&[0x55, 0xAA]) {
        return None;
    }
    let cluster_size = cluster_size_from_boot(&boot, bps)?;
    let total_bytes = le_u64(&boot, 0x28).saturating_mul(bps);
    let serial = le_u64(&boot, 0x48);
    Some(Probe {
        fs_kind: "ntfs",
        confidence: 0.97,
        block_size: u32::try_from(cluster_size).unwrap_or(0),
        label: None, // NTFS label lives in $Volume; read during walk if needed.
        uuid: Some(format!("{serial:016X}")),
        total_bytes,
    })
}

fn cluster_size_from_boot(boot: &[u8], bps: u64) -> Option<u64> {
    let spc_raw = boot.get(13).copied().unwrap_or(0);
    let cluster = if spc_raw <= 0x80 {
        u64::from(spc_raw).max(1) * bps
    } else {
        1u64 << (256 - u32::from(spc_raw))
    };
    if cluster == 0 || cluster > 64 * 1024 * 1024 {
        None
    } else {
        Some(cluster)
    }
}

/// Open an NTFS volume.
pub fn open(src: Arc<dyn BlockSource>, _probe: Probe) -> Result<Ntfs, FsError> {
    let boot = read(&src, 0, 512);
    if boot.get(3..11) != Some(b"NTFS    ") {
        return Err(FsError::NotThisFs("no NTFS OEM id".into()));
    }
    let bps = u64::from(le_u16(&boot, 11));
    let cluster_size = cluster_size_from_boot(&boot, bps)
        .ok_or_else(|| FsError::Corrupt("bad NTFS cluster size".into()))?;
    let mft_cluster = le_u64(&boot, 0x30);
    let mft_offset = mft_cluster.saturating_mul(cluster_size);
    let rec_raw = boot.get(0x40).copied().unwrap_or(0) as i8;
    let mft_record_size = if rec_raw >= 0 {
        (rec_raw as u64).max(1) * cluster_size
    } else {
        1u64 << (rec_raw.unsigned_abs() as u32)
    };
    if !(256..=64 * 1024).contains(&mft_record_size) {
        return Err(FsError::Corrupt("implausible MFT record size".into()));
    }
    let total_bytes = le_u64(&boot, 0x28).saturating_mul(bps);
    let serial = le_u64(&boot, 0x48);

    let mut ntfs = Ntfs {
        src,
        cluster_size,
        mft_offset,
        mft_record_size,
        total_bytes,
        serial,
        mft_extents: Vec::new(),
    };

    // Parse record 0 ($MFT) to learn the MFT's own extents.
    let rec0 = ntfs.read_record_at(mft_offset);
    if let Some(r) = rec0
        .as_ref()
        .and_then(|b| MftRecord::parse(b, mft_record_size))
    {
        if let Some(data) = r.unnamed_data() {
            if !data.resident {
                ntfs.mft_extents = runs_to_extents(&data.runs, cluster_size, data.real_size);
            }
        }
    }
    if ntfs.mft_extents.is_empty() {
        // Fall back to a contiguous MFT of a bounded size.
        ntfs.mft_extents = vec![Extent {
            offset: mft_offset,
            len: total_bytes
                .saturating_sub(mft_offset)
                .min(256 * 1024 * 1024),
        }];
    }
    Ok(ntfs)
}

/// Boxed [`open`] for the engine registry.
pub fn open_boxed(src: Arc<dyn BlockSource>, probe: Probe) -> Result<Box<dyn FileSystem>, FsError> {
    Ok(Box::new(open(src, probe)?))
}

impl Ntfs {
    /// Byte offset of MFT record `n` within the volume (following MFT extents).
    fn record_offset(&self, n: u64) -> Option<u64> {
        let logical = n.saturating_mul(self.mft_record_size);
        let mut acc = 0u64;
        for e in &self.mft_extents {
            if logical < acc + e.len {
                return Some(e.offset + (logical - acc));
            }
            acc += e.len;
        }
        None
    }

    fn read_record_at(&self, off: u64) -> Option<Vec<u8>> {
        let b = read(&self.src, off, self.mft_record_size as usize);
        if b.get(0..4) == Some(FILE_MAGIC.as_slice()) {
            Some(b)
        } else {
            None
        }
    }

    /// Total number of MFT records the extents can hold.
    fn record_count(&self) -> u64 {
        let total: u64 = self.mft_extents.iter().map(|e| e.len).sum();
        total / self.mft_record_size.max(1)
    }

    /// Load `$Bitmap` (MFT record 6) as the cluster allocation bitmap.
    fn load_bitmap(&self) -> Option<Bitmap> {
        let off = self.record_offset(6)?;
        let rec = self.read_record_at(off)?;
        let parsed = MftRecord::parse(&rec, self.mft_record_size)?;
        for a in &parsed.attrs {
            if a.type_id == ATTR_DATA && a.name.is_empty() {
                if a.resident {
                    let bits = a.resident_data(&rec);
                    return Some(Bitmap::new(
                        u32::try_from(self.cluster_size).unwrap_or(0),
                        0,
                        (bits.len() as u64) * 8,
                        bits,
                    ));
                }
                let extents = runs_to_extents(&a.runs, self.cluster_size, a.real_size);
                let mut bits = Vec::new();
                for e in &extents {
                    if bits.len() as u64 >= 64 * 1024 * 1024 {
                        break;
                    }
                    let want = e.len.min(64 * 1024 * 1024) as usize;
                    bits.extend_from_slice(&read(&self.src, e.offset, want));
                }
                let block_count = (bits.len() as u64) * 8;
                return Some(Bitmap::new(
                    u32::try_from(self.cluster_size).unwrap_or(0),
                    0,
                    block_count,
                    bits,
                ));
            }
        }
        let _ = ATTR_BITMAP;
        None
    }
}

/// A record gathered in pass 1 before path resolution.
struct Raw {
    rec_no: u64,
    name: String,
    raw_name: Vec<u8>,
    parent: u64,
    is_dir: bool,
    in_use: bool,
    size: u64,
    extents: Vec<Extent>,
    created: Option<String>,
    modified: Option<String>,
    compressed: bool,
    sparse: bool,
    resident: bool,
}

impl FileSystem for Ntfs {
    fn kind(&self) -> &'static str {
        "ntfs"
    }
    fn block_size(&self) -> u32 {
        u32::try_from(self.cluster_size).unwrap_or(0)
    }
    fn allocation_bitmap(&self) -> Option<Bitmap> {
        self.load_bitmap()
    }

    fn walk(&self, sink: &mut dyn EntrySink, opts: &WalkOpts) -> Result<WalkStats, FsError> {
        let bitmap = self.load_bitmap();
        let mut stats = WalkStats::default();
        let count = self.record_count().min(opts.max_entries as u64 + 64);

        // Pass 1: parse every record into a Raw (keyed by record number), note
        // directories for path resolution, and collect $UsnJrnl:$J and $I30
        // index-allocation extents for best-effort name mining.
        let mut raws: Vec<Raw> = Vec::new();
        let mut dir_names: BTreeMap<u64, (String, u64, bool)> = BTreeMap::new();
        let mut usn_extents: Option<Vec<Extent>> = None;
        let mut i30_extents: Vec<Extent> = Vec::new();
        for n in 0..count {
            let Some(off) = self.record_offset(n) else {
                break;
            };
            let Some(rec) = self.read_record_at(off) else {
                continue;
            };
            let Some(parsed) = MftRecord::parse(&rec, self.mft_record_size) else {
                continue;
            };
            if let Some(raw) = self.to_raw(n, &parsed, &rec) {
                if raw.is_dir && i30_extents.len() < 4096 {
                    if let Some(idx) = parsed.index_allocation() {
                        i30_extents.extend(runs_to_extents(
                            &idx.runs,
                            self.cluster_size,
                            idx.real_size,
                        ));
                    }
                }
                if usn_extents.is_none() && raw.name == "$UsnJrnl" {
                    if let Some(j) = parsed.named_data("$J") {
                        if !j.resident {
                            usn_extents =
                                Some(runs_to_extents(&j.runs, self.cluster_size, j.real_size));
                        }
                    }
                }
                dir_names.insert(raw.rec_no, (raw.name.clone(), raw.parent, raw.is_dir));
                raws.push(raw);
            }
        }

        // Pass 2: resolve paths and emit.
        let mut seen_refs: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        for raw in &raws {
            if stats.emitted as usize >= opts.max_entries {
                break;
            }
            seen_refs.insert(raw.rec_no);
            let include =
                (raw.in_use && opts.include_live) || (!raw.in_use && opts.include_deleted);
            if !include {
                continue;
            }
            // Skip live system metafiles ($MFT, $Bitmap, …) — noise, not user data.
            if raw.in_use && raw.name.starts_with('$') && raw.rec_no < 16 {
                continue;
            }
            let path = resolve_path(raw.rec_no, raw.parent, &raw.name, &dir_names);
            let state = if raw.in_use {
                EntryState::Live
            } else {
                EntryState::Deleted
            };
            let mut e = Entry::new_file(
                raw.rec_no,
                raw.name.clone(),
                raw.raw_name.clone(),
                raw.size,
                state,
            );
            e.kind = if raw.is_dir {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            e.parent_id = Some(raw.parent);
            e.path = Some(path);
            e.created = raw.created.clone();
            e.modified = raw.modified.clone();
            e.extents = raw.extents.clone();
            e.allocated = bitmap
                .as_ref()
                .map(|b| b.is_offset_allocated(raw.extents.first().map(|x| x.offset).unwrap_or(0)))
                .unwrap_or(false);
            // Resident data is always fully present; non-resident deleted data
            // may be overwritten. Compressed/sparse lowers confidence (we do not
            // decompress in round 1).
            let mut conf: f32 = if raw.resident { 0.97 } else { 0.9 };
            if raw.compressed || raw.sparse {
                conf *= 0.6;
            }
            e.confidence = conf;
            match state {
                EntryState::Live => stats.live += 1,
                _ => stats.deleted += 1,
            }
            stats.emitted += 1;
            sink.emit(e);
        }

        // Orphan FILE records (best-effort; after a reformat the MFT is gone but
        // old records survive in unallocated space).
        self.scan_orphans(sink, opts, &mut stats);

        // Best-effort name mining from $UsnJrnl:$J and $I30 index slack: names
        // for files whose MFT record is otherwise gone (docs/plan/04 §3.3).
        self.mine_names(
            sink,
            opts,
            &mut stats,
            usn_extents,
            &i30_extents,
            &seen_refs,
            &dir_names,
        );

        let _ = self.serial;
        Ok(stats)
    }
}

impl Ntfs {
    /// Convert a parsed record into a [`Raw`], or `None` for records without a
    /// name / data we can use.
    fn to_raw(&self, rec_no: u64, parsed: &MftRecord, rec: &[u8]) -> Option<Raw> {
        let (name, raw_name, parent) = parsed.best_file_name()?;
        let (created, modified) = parsed.std_info_dates();
        let data = parsed.unnamed_data();
        let is_dir = parsed.is_directory || data.is_none();
        let (size, extents, compressed, sparse, resident) = match &data {
            Some(d) if d.resident => {
                // Resident data lives inside the MFT record; point the extent at
                // its absolute on-disk location so recover reads it directly.
                let off = parsed.resident_data_offset(rec).unwrap_or(0);
                let len = d.real_size;
                let abs = self.record_offset(rec_no).map(|r| r + off).unwrap_or(0);
                let ext = if len > 0 {
                    vec![Extent { offset: abs, len }]
                } else {
                    Vec::new()
                };
                (len, ext, false, false, true)
            }
            Some(d) => {
                let ext = runs_to_extents(&d.runs, self.cluster_size, d.real_size);
                (d.real_size, ext, d.compressed, d.sparse, false)
            }
            None => (0, Vec::new(), false, false, false),
        };
        Some(Raw {
            rec_no,
            name,
            raw_name,
            parent,
            is_dir,
            in_use: parsed.in_use,
            size,
            extents,
            created,
            modified,
            compressed,
            sparse,
            resident,
        })
    }

    /// Scan the volume for orphan `FILE` records outside the MFT extents.
    fn scan_orphans(&self, sink: &mut dyn EntrySink, opts: &WalkOpts, stats: &mut WalkStats) {
        if !opts.include_deleted {
            return;
        }
        let step = self.mft_record_size.max(1024);
        let mut off = 0u64;
        let mut found = 0u64;
        let in_mft =
            |o: u64, ext: &[Extent]| ext.iter().any(|e| o >= e.offset && o < e.offset + e.len);
        while off + 4 <= self.total_bytes && (stats.emitted as usize) < opts.max_entries {
            if found >= 100_000 {
                break;
            }
            if in_mft(off, &self.mft_extents) {
                off += step;
                continue;
            }
            let head = read(&self.src, off, 4);
            if head.as_slice() == FILE_MAGIC.as_slice() {
                let rec = read(&self.src, off, self.mft_record_size as usize);
                if let Some(parsed) = MftRecord::parse(&rec, self.mft_record_size) {
                    if let Some((name, raw_name, parent)) = parsed.best_file_name() {
                        if !name.starts_with('$') {
                            let data = parsed.unnamed_data();
                            let (size, extents) = match &data {
                                Some(d) if !d.resident => (
                                    d.real_size,
                                    runs_to_extents(&d.runs, self.cluster_size, d.real_size),
                                ),
                                _ => (0, Vec::new()),
                            };
                            let mut e = Entry::new_file(
                                0xFFFF_0000_0000_0000 | off,
                                name,
                                raw_name,
                                size,
                                EntryState::Orphaned,
                            );
                            e.parent_id = Some(parent);
                            e.extents = extents;
                            e.confidence = 0.55;
                            stats.deleted += 1;
                            stats.emitted += 1;
                            found += 1;
                            sink.emit(e);
                        }
                    }
                }
            }
            off += step;
        }
    }

    /// Read a bounded concatenation of `extents` into one buffer.
    fn read_extents_bytes(&self, extents: &[Extent], budget: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut left = budget;
        for e in extents {
            if left == 0 {
                break;
            }
            let want = e.len.min(left);
            out.extend_from_slice(&read(&self.src, e.offset, want as usize));
            left = left.saturating_sub(want);
        }
        out
    }

    /// Best-effort: mine names from `$UsnJrnl:$J` and `$I30` slack, emitting a
    /// name-only `Historical` entry for any referenced file we did not already
    /// recover. Bounded by a byte budget and an emission cap.
    #[allow(clippy::too_many_arguments)]
    fn mine_names(
        &self,
        sink: &mut dyn EntrySink,
        opts: &WalkOpts,
        stats: &mut WalkStats,
        usn_extents: Option<Vec<Extent>>,
        i30_extents: &[Extent],
        seen_refs: &std::collections::BTreeSet<u64>,
        dirs: &BTreeMap<u64, (String, u64, bool)>,
    ) {
        if !opts.include_deleted {
            return;
        }
        let mut mined: Vec<usn::MinedName> = Vec::new();
        if let Some(ext) = usn_extents {
            let data = self.read_extents_bytes(&ext, 32 * 1024 * 1024);
            mined.extend(usn::parse_usn_stream(&data));
        }
        if !i30_extents.is_empty() {
            let data = self.read_extents_bytes(i30_extents, 16 * 1024 * 1024);
            mined.extend(usn::scan_indx_names(&data));
        }
        let mut emitted = 0usize;
        let mut seen_here: std::collections::BTreeSet<(u64, String)> =
            std::collections::BTreeSet::new();
        for m in mined {
            if emitted >= 10_000 || stats.emitted as usize >= opts.max_entries {
                break;
            }
            if seen_refs.contains(&m.file_ref) {
                continue; // already recovered with its own record
            }
            if !seen_here.insert((m.file_ref, m.name.clone())) {
                continue;
            }
            let path = resolve_path(m.file_ref, m.parent, &m.name, dirs);
            let mut e = Entry::new_file(
                m.file_ref,
                m.name.clone(),
                m.name.as_bytes().to_vec(),
                0,
                EntryState::Historical,
            );
            e.parent_id = Some(m.parent);
            e.path = Some(path);
            e.confidence = 0.4; // name known, content not located
            stats.deleted += 1;
            stats.emitted += 1;
            emitted += 1;
            sink.emit(e);
        }
    }
}

/// Resolve a full path by walking the parent chain (bounded).
fn resolve_path(
    rec_no: u64,
    parent: u64,
    name: &str,
    dirs: &BTreeMap<u64, (String, u64, bool)>,
) -> String {
    let mut parts = vec![name.to_string()];
    let mut cur = parent;
    let mut guard = 0;
    while cur != ROOT_REF && cur != rec_no && guard < 256 {
        guard += 1;
        match dirs.get(&cur) {
            Some((pname, pparent, _)) if !pname.is_empty() => {
                parts.push(pname.clone());
                cur = *pparent;
            }
            _ => break,
        }
    }
    parts.reverse();
    parts.join("/")
}

/// Convert NTFS data runs to absolute byte extents, clamped to `real_size`.
fn runs_to_extents(runs: &[record::Run], cluster_size: u64, real_size: u64) -> Vec<Extent> {
    let mut out = Vec::new();
    let mut remaining = real_size;
    for r in runs {
        if remaining == 0 {
            break;
        }
        if r.sparse {
            // A hole contributes zero bytes we can recover; skip but consume size.
            let span = r.length.saturating_mul(cluster_size);
            remaining = remaining.saturating_sub(span.min(remaining));
            continue;
        }
        let offset = r.lcn.saturating_mul(cluster_size);
        let span = r.length.saturating_mul(cluster_size);
        let take = span.min(remaining);
        out.push(Extent { offset, len: take });
        remaining = remaining.saturating_sub(take);
    }
    out
}

// ---- byte helpers ----

fn read(src: &Arc<dyn BlockSource>, offset: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let total = src.len();
    if len == 0 || offset >= total {
        return out;
    }
    let ss = u64::from(src.sector_size().max(1));
    let aligned = offset - (offset % ss);
    let end = offset.saturating_add(len as u64).min(total);
    let aligned_end = end.div_ceil(ss).saturating_mul(ss);
    let span = (aligned_end - aligned) as usize;
    let mut buf = vec![0u8; span];
    let _ = src.read_at(aligned, &mut buf);
    let skip = (offset - aligned) as usize;
    let avail = span.saturating_sub(skip);
    let copy = len.min(avail).min((end - offset) as usize);
    if let (Some(d), Some(s)) = (out.get_mut(..copy), buf.get(skip..skip + copy)) {
        d.copy_from_slice(s);
    }
    out
}

pub(crate) fn le_u16(b: &[u8], off: usize) -> u16 {
    match b.get(off..off + 2) {
        Some(s) => u16::from_le_bytes([
            s.first().copied().unwrap_or(0),
            s.get(1).copied().unwrap_or(0),
        ]),
        None => 0,
    }
}
pub(crate) fn le_u32(b: &[u8], off: usize) -> u32 {
    match b.get(off..off + 4) {
        Some(s) => {
            let mut a = [0u8; 4];
            a.copy_from_slice(s);
            u32::from_le_bytes(a)
        }
        None => 0,
    }
}
pub(crate) fn le_u64(b: &[u8], off: usize) -> u64 {
    match b.get(off..off + 8) {
        Some(s) => {
            let mut a = [0u8; 8];
            a.copy_from_slice(s);
            u64::from_le_bytes(a)
        }
        None => 0,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn runs_to_extents_basic() {
        let runs = vec![
            record::Run {
                lcn: 10,
                length: 2,
                sparse: false,
            },
            record::Run {
                lcn: 20,
                length: 3,
                sparse: false,
            },
        ];
        let ex = runs_to_extents(&runs, 4096, 2 * 4096 + 4000);
        assert_eq!(ex.len(), 2);
        assert_eq!(ex[0].offset, 10 * 4096);
        assert_eq!(ex[0].len, 2 * 4096);
        assert_eq!(ex[1].offset, 20 * 4096);
        assert_eq!(ex[1].len, 4000);
    }

    #[test]
    fn path_resolution() {
        let mut dirs = BTreeMap::new();
        dirs.insert(5u64, ("".to_string(), 5u64, true)); // root
        dirs.insert(64u64, ("Users".to_string(), 5u64, true));
        dirs.insert(65u64, ("me".to_string(), 64u64, true));
        let p = resolve_path(100, 65, "photo.jpg", &dirs);
        assert_eq!(p, "Users/me/photo.jpg");
    }
}
