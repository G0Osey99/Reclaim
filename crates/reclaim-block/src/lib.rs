//! Read-only block layer for Reclaim (docs/plan/03-architecture.md §2.1).
//!
//! Everything above this crate is platform-independent and testable against
//! image files, which is what makes CI practical. The central abstraction is
//! [`BlockSource`]: a random-access, read-only view of a device or image that
//! reports per-sector read status so higher layers can continue past bad
//! sectors instead of aborting.
//!
//! # Read-only by construction
//! [`BlockSource`] has **no write method**. The imager (Phase 1) writes to a
//! destination `File`, never through this trait. `scripts/check-readonly.sh`
//! statically enforces that no write/mutate syscall appears outside the
//! reviewed allow-list (build guide Part 3.2).
//!
//! # Parser-style lints
//! This crate handles raw on-disk bytes and untrusted geometry, so it denies
//! `unwrap`/`expect`/indexing per build guide Part 1.4 rule 3. Test modules opt
//! out locally where panics are the point of the assertion.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_op_in_unsafe_fn)]

mod bad_block;
mod bitset;
mod cache;
pub mod container;
mod error;
mod fault;
mod image_file;
pub mod imaging;
mod memory;
mod offset_view;
pub mod open_log;
mod raw_device;
mod source;

pub use bad_block::{BadBlockMap, LbaRange};
pub use bitset::Bitset;
pub use cache::ReadAheadCache;
pub use container::{ConcatSource, Container, MappedSource};
pub use error::BlockError;
pub use fault::FaultInjector;
pub use image_file::ImageFile;
pub use memory::MemorySource;
pub use offset_view::OffsetView;
pub use raw_device::RawDevice;
pub use source::{BlockSource, ReadResult, SectorStatus, SourceId};

use std::path::Path;
use std::sync::Arc;

/// Open an image path, auto-detecting image-container formats (DMG, VMDK, VDI,
/// VHD/VHDX, QCOW2, EnCase E01, sparseimage, split sets — docs/plan/04 §5).
/// Unrecognized files fall back to a raw [`ImageFile`].
pub fn open_image_auto(path: impl AsRef<Path>) -> Result<Arc<dyn BlockSource>, BlockError> {
    let path = path.as_ref();
    if let Some(src) = container::open_container(path)? {
        return Ok(src);
    }
    Ok(Arc::new(ImageFile::open(path)?))
}

/// The detected container format for `path` (for `reclaim info`).
#[must_use]
pub fn detect_container(path: impl AsRef<Path>) -> Container {
    container::detect(path.as_ref())
}

/// Size of a read-ahead cache chunk: 1 MiB (docs/plan/03 §2.1).
pub const CHUNK_SIZE: u64 = 1024 * 1024;

/// Default logical sector size assumed when a source cannot report one.
pub const DEFAULT_SECTOR_SIZE: u32 = 512;
