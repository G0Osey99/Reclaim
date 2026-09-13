//! FFI value types (UniFFI records/enums) and conversions from the core types.
//! These are plain data marshalled to Swift; behaviour lives on the `Session`
//! and `ImageJob` objects and the free functions.

use reclaim_session::store::CarvedRecord;
use reclaim_session::QueryFilter;

/// One enumerated source (device / partition / volume), from a
/// [`reclaim_platform_macos::Disk`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct SourceSummary {
    pub bsd_name: String,
    pub raw_node: String,
    pub whole_disk: String,
    pub size: u64,
    pub whole: bool,
    pub removable: bool,
    pub internal: Option<bool>,
    pub bus: Option<String>,
    pub model: Option<String>,
    pub volume_name: Option<String>,
    pub content: Option<String>,
    pub fs_kind: Option<String>,
    pub mount_point: Option<String>,
    pub is_apfs_container: bool,
    pub filevault: Option<bool>,
    /// SMART label (`ok` / `FAILING` / `n/a` / `—`).
    pub smart: String,
}

impl From<&reclaim_platform_macos::Disk> for SourceSummary {
    fn from(d: &reclaim_platform_macos::Disk) -> Self {
        SourceSummary {
            bsd_name: d.bsd_name.clone(),
            raw_node: d.raw_device_node.clone(),
            whole_disk: d.whole_disk.clone(),
            size: d.size,
            whole: d.whole,
            removable: d.removable,
            internal: d.internal,
            bus: d.bus.clone(),
            model: d.model.clone(),
            volume_name: d.volume_name.clone(),
            content: d.content.clone(),
            fs_kind: d.fs_kind.clone(),
            mount_point: d.mount_point.clone(),
            is_apfs_container: d.is_apfs_container(),
            filevault: d.apfs.as_ref().and_then(|a| a.filevault),
            smart: d.smart.label().to_string(),
        }
    }
}

/// Coarse drive-health level (doc 08 §2.2 health chip).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HealthLevel {
    Good,
    Warn,
    Bad,
    Unknown,
}

impl From<reclaim_platform_macos::HealthVerdict> for HealthLevel {
    fn from(v: reclaim_platform_macos::HealthVerdict) -> Self {
        use reclaim_platform_macos::HealthVerdict as H;
        match v {
            H::Good => HealthLevel::Good,
            H::Warn => HealthLevel::Warn,
            H::Bad => HealthLevel::Bad,
            H::Unknown => HealthLevel::Unknown,
        }
    }
}

/// A partition in the source's partition map.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PartitionInfo {
    pub index: u32,
    pub type_label: String,
    pub kind: String,
    pub start: u64,
    pub len: u64,
    pub name: Option<String>,
}

/// Result of probing a source (doc 08 §2.2 source detail).
#[derive(Debug, Clone, uniffi::Record)]
pub struct SourceProbe {
    pub source: String,
    pub size: u64,
    pub sector_size_logical: u32,
    pub sector_size_physical: u32,
    pub read_speed_bps: f64,
    pub probe_bad_sectors: u64,
    pub health: HealthLevel,
    pub smart: String,
    pub image_first: bool,
    pub scheme: String,
    pub partitions: Vec<PartitionInfo>,
    pub warnings: Vec<String>,
    pub container: Option<String>,
}

/// Scan configuration (doc 08 §2.2 scan plan).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScanOptions {
    /// Run the metadata (named-file) pass.
    pub quick: bool,
    /// Run the carve (deep) pass.
    pub deep: bool,
    /// Restrict carve to these families (empty = all).
    pub families: Vec<String>,
    /// Restrict carve to these signature ids (empty = all).
    pub sig_ids: Vec<String>,
    /// Carve block size (0 = auto).
    pub block_size: u32,
    /// Byte-granular matching in non-uniform blocks.
    pub brute_force: bool,
    /// Keep Truncated/Suspect carved results.
    pub keep_corrupted: bool,
    /// Cap on any single carved file.
    pub max_file_size: u64,
    /// Optional byte range (start,end) to scan.
    pub range_start: Option<u64>,
    pub range_end: Option<u64>,
    /// Hide allocated carved results (needs the FS bitmap).
    pub unallocated_only: bool,
    /// Checkpoint interval seconds.
    pub checkpoint_secs: u64,
    /// Continue an interrupted scan.
    pub resume: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            quick: true,
            deep: true,
            families: Vec::new(),
            sig_ids: Vec::new(),
            block_size: 0,
            brute_force: false,
            keep_corrupted: true,
            max_file_size: 4 * 1024 * 1024 * 1024,
            range_start: None,
            range_end: None,
            unallocated_only: false,
            checkpoint_secs: 5,
            resume: false,
        }
    }
}

/// A contiguous byte extent.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Extent {
    pub offset: u64,
    pub len: u64,
}

/// A single result row for the GUI (from a [`CarvedRecord`]).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ResultRecord {
    pub id: String,
    pub engine: String,
    pub source_kind: String,
    pub offset: u64,
    pub len: u64,
    pub format: String,
    pub family: String,
    pub ext: String,
    pub validity: String,
    pub score: u8,
    pub block_aligned: bool,
    pub name: Option<String>,
    pub date: Option<String>,
    pub model: Option<String>,
    pub has_thumbnail: bool,
    pub path: String,
    pub state: Option<String>,
    pub kind: String,
    pub extents: Vec<Extent>,
    pub merged: bool,
}

impl From<&CarvedRecord> for ResultRecord {
    fn from(r: &CarvedRecord) -> Self {
        let source_kind = match r.engine.as_str() {
            "carve" => "carved",
            _ => {
                if r.path.is_some() {
                    "named"
                } else {
                    "reconstructed"
                }
            }
        }
        .to_string();
        ResultRecord {
            id: r.id.clone(),
            engine: r.engine.clone(),
            source_kind,
            offset: r.offset,
            len: r.len,
            format: r.format.clone(),
            family: r.family.clone(),
            ext: r.ext.clone(),
            validity: r.validity.clone(),
            score: r.score,
            block_aligned: r.block_aligned,
            name: r.name.clone(),
            date: r.date.clone(),
            model: r.model.clone(),
            has_thumbnail: r.thumb_offset.is_some() && r.thumb_len.is_some(),
            path: r.synth_path(),
            state: r.state.clone(),
            kind: r.kind.clone(),
            extents: r
                .extents()
                .into_iter()
                .map(|(offset, len)| Extent { offset, len })
                .collect(),
            merged: r.merged,
        }
    }
}

/// A page of results plus the total matching the filter (doc 08 §4 paging).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ResultPage {
    pub total: u64,
    pub offset: u32,
    pub items: Vec<ResultRecord>,
}

/// Sort key for a query.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum SortKey {
    Offset,
    Size,
    Date,
    Score,
    Path,
}

/// The GUI filter rail (doc 08 §2.3), mapped to a [`QueryFilter`].
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ResultFilter {
    pub family: Option<String>,
    pub exts: Vec<String>,
    pub min_size: Option<u64>,
    pub after: Option<String>,
    pub min_score: Option<u8>,
    pub engine: Option<String>,
    pub full_only: bool,
    pub deleted_only: bool,
    pub include_merged: bool,
    pub path_glob: Option<String>,
    pub sort: Option<SortKey>,
}

impl ResultFilter {
    /// Build a [`QueryFilter`] with the given paging window.
    pub(crate) fn to_query(&self, offset: u32, limit: u32) -> QueryFilter {
        QueryFilter {
            family: self.family.clone(),
            exts: self.exts.clone(),
            min_size: self.min_size,
            after: self.after.clone(),
            min_score: self.min_score,
            engine: self.engine.clone(),
            full_only: self.full_only,
            deleted_only: self.deleted_only,
            include_merged: self.include_merged,
            path_glob: self.path_glob.clone(),
            ids: None,
            sort: self.sort.map(|s| match s {
                SortKey::Offset => reclaim_session::Sort::Offset,
                SortKey::Size => reclaim_session::Sort::Size,
                SortKey::Date => reclaim_session::Sort::Date,
                SortKey::Score => reclaim_session::Sort::Score,
                SortKey::Path => reclaim_session::Sort::Path,
            }),
            limit: if limit == 0 {
                None
            } else {
                Some(limit as usize)
            },
            offset: if offset == 0 {
                None
            } else {
                Some(offset as usize)
            },
        }
    }

    /// A filter for counting (no paging).
    pub(crate) fn to_count_query(&self) -> QueryFilter {
        let mut q = self.to_query(0, 0);
        q.limit = None;
        q.offset = None;
        q
    }
}

/// `(family, count)` for the filter rail.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FamilyCount {
    pub family: String,
    pub count: u64,
}

/// What kind of preview to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PreviewKind {
    /// Downscaled PNG thumbnail (raster / embedded JPEG).
    Thumbnail,
    /// Full-resolution PNG (raster / embedded JPEG).
    Image,
    /// UTF-8 text.
    Text,
    /// Hex dump.
    Hex,
}

/// A rendered preview.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PreviewResult {
    /// MIME type of `bytes` (`image/png`, `text/plain`).
    pub mime: String,
    /// The preview payload.
    pub bytes: Vec<u8>,
}

/// Name-collision policy for recovery.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum CollisionPolicy {
    Rename,
    Skip,
    Overwrite,
}

impl From<CollisionPolicy> for reclaim_session::Collision {
    fn from(c: CollisionPolicy) -> Self {
        match c {
            CollisionPolicy::Rename => reclaim_session::Collision::Rename,
            CollisionPolicy::Skip => reclaim_session::Collision::Skip,
            CollisionPolicy::Overwrite => reclaim_session::Collision::Overwrite,
        }
    }
}

/// Recovery options (doc 08 §2.4 recover sheet).
#[derive(Debug, Clone, uniffi::Record)]
pub struct RecoverOptions {
    pub dest: String,
    pub preserve_paths: bool,
    pub flat: bool,
    pub collision: CollisionPolicy,
    pub verify: bool,
    pub allow_same_device: bool,
}

/// A recovered file (per-file line of the recover result).
#[derive(Debug, Clone, uniffi::Record)]
pub struct RecoveredEntry {
    pub id: String,
    pub path: String,
    pub len: u64,
    pub validity: String,
    pub blake3: String,
    pub verified: Option<bool>,
}

/// Summary of a recover run.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RecoverOutcome {
    pub recovered: u64,
    pub skipped: u64,
    pub partial: u64,
    pub verify_failed: u64,
    pub bytes: u64,
    pub manifest_path: String,
    pub dest: String,
    pub files: Vec<RecoveredEntry>,
}

/// Live validation of a recover destination (doc 08 §2.4).
#[derive(Debug, Clone, uniffi::Record)]
pub struct DestinationCheck {
    /// True if the destination is safe (different disk, enough space).
    pub ok: bool,
    /// True if the destination is on the same physical disk as the source.
    pub same_disk: bool,
    /// Free bytes on the destination filesystem, if known.
    pub free_bytes: Option<u64>,
    /// True if free space is below the selected size × 1.1.
    pub low_space: bool,
    /// Human-readable reason when `ok` is false.
    pub reason: Option<String>,
}

/// Byte-to-byte imaging options (doc 08 §2.5).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ImageOptions {
    pub chunk_size: u32,
    pub retries: u32,
    pub reverse_pass: bool,
    pub sparse: bool,
    pub zstd: bool,
    /// `blake3` or `sha256`.
    pub hash: String,
    pub resume: bool,
}

impl Default for ImageOptions {
    fn default() -> Self {
        ImageOptions {
            chunk_size: 0,
            retries: 3,
            reverse_pass: true,
            sparse: false,
            zstd: false,
            hash: "blake3".to_string(),
            resume: false,
        }
    }
}

/// Outcome of an imaging job (doc 08 §2.5).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ImageDone {
    pub complete: bool,
    pub had_bad: bool,
    pub bad_sectors: u64,
    pub whole_hash: Option<String>,
    pub image_path: String,
    pub map_path: String,
}

/// A lost-volume proposal (doc 08 §2.6).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ProposalInfo {
    pub index: u32,
    pub fs: String,
    pub start: u64,
    pub len: u64,
    pub confidence: f64,
    pub evidence: String,
    /// The `session:<dir>/volume/<n>` spec to adopt/scan.
    pub source_spec: String,
}

/// A recovery point (APFS snapshot or reachable checkpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SnapshotPointKind {
    Snapshot,
    Checkpoint,
}

/// One snapshot/checkpoint (doc 08 §2.8).
#[derive(Debug, Clone, uniffi::Record)]
pub struct SnapshotInfo {
    pub kind: SnapshotPointKind,
    pub xid: u64,
    pub name: String,
}

/// The APFS snapshot browser payload.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SnapshotList {
    pub volumes: Vec<String>,
    pub recoverable_xids: Vec<u64>,
    pub points: Vec<SnapshotInfo>,
}

/// A file present at a snapshot but absent now (snapshot diff).
#[derive(Debug, Clone, uniffi::Record)]
pub struct DiffEntry {
    pub path: String,
    pub size: u64,
    pub state: String,
}

/// Report format (doc 08 §2.9).
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum ReportFormat {
    Json,
    Html,
    Csv,
}

impl From<ReportFormat> for reclaim_report::Format {
    fn from(f: ReportFormat) -> Self {
        match f {
            ReportFormat::Json => reclaim_report::Format::Json,
            ReportFormat::Html => reclaim_report::Format::Html,
            ReportFormat::Csv => reclaim_report::Format::Csv,
        }
    }
}

/// Result of a completed scan run (returned by `Session::wait`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScanSummary {
    pub found: u64,
    pub interrupted: bool,
    pub elapsed: f64,
    pub complete: bool,
    pub vanished: bool,
    pub total_rows: u64,
}

/// A live event delivered to the Swift [`crate::EventSink`] during a scan,
/// recover or image job (doc 08 §5). Mirrors `reclaim_session::Event` plus the
/// imaging/recover progress shapes that have no session event.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum RcEvent {
    Progress {
        pass: String,
        cursor: u64,
        pct: f64,
        rate: u64,
    },
    Found {
        id: String,
        engine: String,
        path: String,
        size: u64,
        score: u8,
    },
    Volume {
        start: u64,
        len: u64,
        fs: String,
        confidence: f64,
    },
    ReadError {
        lba: u64,
    },
    Warning {
        msg: String,
    },
    PassComplete {
        pass: String,
    },
    Done {
        found: u64,
        elapsed: f64,
        interrupted: bool,
    },
    ImageProgress {
        pass: u8,
        done: u64,
        total: u64,
        bad_sectors: u64,
    },
    RecoverProgress {
        done: u64,
        total: u64,
        path: String,
    },
}

/// Translate a core scan event to the FFI event.
#[must_use]
pub fn to_rc_event(ev: &reclaim_session::Event) -> RcEvent {
    use reclaim_session::Event as E;
    match ev {
        E::Progress {
            pass,
            lba,
            pct,
            rate,
        } => RcEvent::Progress {
            pass: pass.clone(),
            cursor: *lba,
            pct: *pct,
            rate: *rate,
        },
        E::Found {
            id,
            engine,
            path,
            size,
            score,
        } => RcEvent::Found {
            id: id.clone(),
            engine: engine.clone(),
            path: path.clone(),
            size: *size,
            score: *score,
        },
        E::ReadError { lba } => RcEvent::ReadError { lba: *lba },
        E::Volume {
            start,
            len,
            fs,
            confidence,
        } => RcEvent::Volume {
            start: *start,
            len: *len,
            fs: fs.clone(),
            confidence: f64::from(*confidence),
        },
        E::Warning { msg } => RcEvent::Warning { msg: msg.clone() },
        E::PassComplete { pass } => RcEvent::PassComplete { pass: pass.clone() },
        E::Done {
            found,
            elapsed,
            interrupted,
        } => RcEvent::Done {
            found: *found,
            elapsed: *elapsed,
            interrupted: *interrupted,
        },
    }
}
