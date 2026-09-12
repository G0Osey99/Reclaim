//! Reclaim filesystem-engine core (docs/plan/03 §2.2).
//!
//! Every `fs-*` crate (fs-exfat, fs-fat, fs-ntfs, and the Phase-3/4 engines)
//! implements the [`FileSystem`] trait and emits [`Entry`] values through an
//! [`EntrySink`]. An `Entry` carries both a best-effort UTF-8 [`Entry::name`]
//! and the exact [`Entry::raw_name`] bytes, a lifecycle [`EntryState`]
//! (`Live | Deleted | Orphaned | Historical`), a structural [`Entry::confidence`]
//! and the file's on-disk [`Extent`]s.
//!
//! # Coordinate systems
//! An engine is opened over a [`reclaim_block::BlockSource`] that is *the
//! volume* (partition schemes are peeled off by `reclaim-part`, which hands the
//! engine an `OffsetView`). Engines therefore emit **volume-relative** extents;
//! the session layer shifts them to absolute source offsets via an
//! [`EntrySink`] that knows the volume base, so carve results (absolute) and
//! named entries (absolute) share one coordinate space for the merge
//! (docs/plan/03 §2.3 / §5 step 6).
//!
//! # No panics on data
//! These engines parse hostile on-disk bytes, so this crate and its dependents
//! deny panicking accessors (build guide Part 1.4 rule 3).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

mod bitmap;
pub mod score;

pub use bitmap::Bitmap;

use std::sync::Arc;

/// Engine errors. Corruption is the normal input, so these are ordinary values
/// (docs/plan/03 §6): a parser returns `Err` rather than panicking.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// The source did not look like this filesystem (probe/open mismatch).
    #[error("not this filesystem: {0}")]
    NotThisFs(String),
    /// A structure was too corrupt to continue (bounded; never a panic).
    #[error("corrupt structure: {0}")]
    Corrupt(String),
    /// An explicit work/recursion budget was exhausted (DoS guard).
    #[error("budget exhausted: {0}")]
    Budget(String),
}

/// The kind of a directory entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symlink / reparse point / other non-regular node.
    Other,
}

impl EntryKind {
    /// Lowercase label for output/storage.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            EntryKind::File => "file",
            EntryKind::Dir => "dir",
            EntryKind::Other => "other",
        }
    }
}

/// Lifecycle state of a recovered entry (docs/plan/03 §2.2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EntryState {
    /// Present and in use in the current filesystem.
    Live,
    /// Marked deleted but its directory record still carries name + extents.
    Deleted,
    /// Found by scanning (no reachable parent directory) — e.g. an orphan
    /// NTFS `FILE` record after a reformat.
    Orphaned,
    /// Recovered from an older consistent view (APFS checkpoint/snapshot,
    /// journal) — Phase 3+.
    Historical,
}

impl EntryState {
    /// Lowercase label for output/storage.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            EntryState::Live => "live",
            EntryState::Deleted => "deleted",
            EntryState::Orphaned => "orphaned",
            EntryState::Historical => "historical",
        }
    }

    /// True for states a recovery user usually wants (`--deleted-only`).
    #[must_use]
    pub fn is_recoverable_target(self) -> bool {
        matches!(
            self,
            EntryState::Deleted | EntryState::Orphaned | EntryState::Historical
        )
    }
}

/// A contiguous byte run of a file on the source. Offsets are volume-relative
/// as emitted by an engine, and absolute once shifted by an [`EntrySink`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Extent {
    /// Byte offset of the run start.
    pub offset: u64,
    /// Run length in bytes.
    pub len: u64,
}

impl Extent {
    /// One-past-the-end offset (saturating).
    #[must_use]
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(self.len)
    }
}

/// An ordered list of a file's extents.
pub type ExtentList = Vec<Extent>;

/// A recovered filesystem entry (docs/plan/03 §2.2).
#[derive(Clone, Debug)]
pub struct Entry {
    /// Engine-local identifier (exFAT/FAT: first-cluster or set offset; NTFS:
    /// MFT record number). Unique within one walk.
    pub id: u64,
    /// Parent directory's `id`, if known.
    pub parent_id: Option<u64>,
    /// Best-effort UTF-8 name (lossy where the raw bytes are not valid UTF-8).
    pub name: String,
    /// Raw on-disk name bytes (UTF-16LE decoded to bytes, or 8.3, …), preserved
    /// verbatim so nothing is silently lost.
    pub raw_name: Vec<u8>,
    /// File / directory / other.
    pub kind: EntryKind,
    /// Logical size in bytes (the file's data length).
    pub size: u64,
    /// Full path relative to the volume root, filled in by the engine once the
    /// directory tree is linked. `None` if the parent chain is unknown.
    pub path: Option<String>,
    /// Creation time, `YYYY-MM-DD` (best effort).
    pub created: Option<String>,
    /// Modification time, `YYYY-MM-DD` (best effort).
    pub modified: Option<String>,
    /// The file's data extents (volume-relative as emitted).
    pub extents: ExtentList,
    /// Lifecycle state.
    pub state: EntryState,
    /// Structural confidence 0..1 (how sure the engine is this record is real
    /// and its extents are right — feeds the recoverability score).
    pub confidence: f32,
    /// Whether this entry's extents are currently marked allocated in the live
    /// bitmap (allocated ⇒ likely overwritten for a deleted file).
    pub allocated: bool,
    /// The chain was lost/zeroed and the extents were assumed contiguous
    /// (docs/plan/04 §3.4) — the result must be marked `Suspect`.
    pub contiguous_assumed: bool,
}

impl Entry {
    /// Create a bare file entry with the required fields; callers fill the rest.
    #[must_use]
    pub fn new_file(
        id: u64,
        name: String,
        raw_name: Vec<u8>,
        size: u64,
        state: EntryState,
    ) -> Self {
        Entry {
            id,
            parent_id: None,
            name,
            raw_name,
            kind: EntryKind::File,
            size,
            path: None,
            created: None,
            modified: None,
            extents: Vec::new(),
            state,
            confidence: 0.5,
            allocated: false,
            contiguous_assumed: false,
        }
    }

    /// Total bytes covered by the entry's extents.
    #[must_use]
    pub fn extent_bytes(&self) -> u64 {
        self.extents
            .iter()
            .fold(0u64, |a, e| a.saturating_add(e.len))
    }
}

/// A read-only snapshot reference (APFS/Btrfs/ZFS — Phase 3+). Present so the
/// trait is stable; the Phase-2 engines return an empty list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotRef {
    /// Engine-specific snapshot id / transaction id.
    pub id: u64,
    /// Human label (snapshot name or xid).
    pub label: String,
}

/// Result of a [`FileSystem::probe`]-style check (docs/plan/03 §2.2).
#[derive(Clone, Debug)]
pub struct Probe {
    /// Filesystem kind, e.g. `"exfat"`, `"fat32"`, `"ntfs"`.
    pub fs_kind: &'static str,
    /// Confidence the source is this filesystem, 0..1.
    pub confidence: f32,
    /// Cluster/block size in bytes.
    pub block_size: u32,
    /// Volume label, if present.
    pub label: Option<String>,
    /// Volume serial / UUID string, if present.
    pub uuid: Option<String>,
    /// Total volume size in bytes (as the superblock declares).
    pub total_bytes: u64,
}

/// Options for [`FileSystem::walk`].
#[derive(Copy, Clone, Debug)]
pub struct WalkOpts {
    /// Emit live (in-use) entries.
    pub include_live: bool,
    /// Emit deleted / orphaned entries.
    pub include_deleted: bool,
    /// Hard cap on entries emitted (DoS guard on a crafted image).
    pub max_entries: usize,
    /// Include the sealed APFS System volume (skipped by default — it holds no
    /// user data; docs/plan/06 §2). Engines that have no such concept ignore it.
    pub include_system_volume: bool,
}

impl Default for WalkOpts {
    fn default() -> Self {
        WalkOpts {
            include_live: true,
            include_deleted: true,
            max_entries: 5_000_000,
            include_system_volume: false,
        }
    }
}

/// Statistics returned by a walk.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WalkStats {
    /// Entries emitted.
    pub emitted: u64,
    /// Live entries seen.
    pub live: u64,
    /// Deleted/orphaned entries seen.
    pub deleted: u64,
}

/// Where a [`FileSystem::walk`] sends entries. The session implements this to
/// shift extents to absolute offsets and batch-insert into SQLite; tests use a
/// `Vec` collector.
pub trait EntrySink {
    /// Accept one recovered entry.
    fn emit(&mut self, entry: Entry);
}

/// Collect entries into a `Vec` (tests / in-memory callers).
#[derive(Debug, Default)]
pub struct VecSink {
    /// The collected entries.
    pub entries: Vec<Entry>,
}

impl EntrySink for VecSink {
    fn emit(&mut self, entry: Entry) {
        self.entries.push(entry);
    }
}

/// An opened filesystem metadata engine (docs/plan/03 §2.2). Object-safe so the
/// planner can hold `Box<dyn FileSystem>` from any `fs-*` crate; construction is
/// a per-crate `probe`/`open` pair (probe returns [`Probe`] with confidence,
/// open consumes it).
pub trait FileSystem: std::fmt::Debug + Send {
    /// Filesystem kind label (the engine name stored on results).
    fn kind(&self) -> &'static str;

    /// Cluster/block size in bytes.
    fn block_size(&self) -> u32;

    /// The allocation bitmap (used/free blocks), if the engine exposes one.
    /// Feeds the carver's `--unallocated-only` and the recoverability score.
    fn allocation_bitmap(&self) -> Option<Bitmap>;

    /// Enumerate live + deleted entries, emitting each to `sink`.
    fn walk(&self, sink: &mut dyn EntrySink, opts: &WalkOpts) -> Result<WalkStats, FsError>;

    /// The data extents of `entry` (volume-relative). The Phase-2 engines fill
    /// `entry.extents` during [`FileSystem::walk`]; this returns them (and is
    /// the hook for lazily-resolving engines later).
    fn read_extents(&self, entry: &Entry) -> Result<ExtentList, FsError> {
        Ok(entry.extents.clone())
    }

    /// Older consistent views (APFS/Btrfs/ZFS — Phase 3+); empty here.
    fn snapshots(&self) -> Vec<SnapshotRef> {
        Vec::new()
    }
}

/// A probe function exported by each `fs-*` crate.
pub type ProbeFn = fn(&Arc<dyn reclaim_block::BlockSource>) -> Option<Probe>;
/// An open function exported by each `fs-*` crate (returns a boxed engine).
pub type OpenFn =
    fn(Arc<dyn reclaim_block::BlockSource>, Probe) -> Result<Box<dyn FileSystem>, FsError>;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn entry_extent_math() {
        let mut e = Entry::new_file(1, "x".into(), b"x".to_vec(), 100, EntryState::Deleted);
        e.extents = vec![
            Extent { offset: 0, len: 64 },
            Extent {
                offset: 4096,
                len: 36,
            },
        ];
        assert_eq!(e.extent_bytes(), 100);
        assert_eq!(e.extents[0].end(), 64);
    }

    #[test]
    fn state_targets() {
        assert!(EntryState::Deleted.is_recoverable_target());
        assert!(EntryState::Orphaned.is_recoverable_target());
        assert!(!EntryState::Live.is_recoverable_target());
    }

    #[test]
    fn vec_sink_collects() {
        let mut s = VecSink::default();
        s.emit(Entry::new_file(
            1,
            "a".into(),
            b"a".to_vec(),
            1,
            EntryState::Live,
        ));
        assert_eq!(s.entries.len(), 1);
    }
}
