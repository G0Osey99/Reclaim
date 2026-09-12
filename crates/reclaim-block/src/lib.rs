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
mod error;
mod fault;
mod image_file;
mod offset_view;
pub mod open_log;
mod raw_device;
mod source;

pub use bad_block::{BadBlockMap, LbaRange};
pub use bitset::Bitset;
pub use cache::ReadAheadCache;
pub use error::BlockError;
pub use fault::FaultInjector;
pub use image_file::ImageFile;
pub use offset_view::OffsetView;
pub use raw_device::RawDevice;
pub use source::{BlockSource, ReadResult, SectorStatus, SourceId};

/// Size of a read-ahead cache chunk: 1 MiB (docs/plan/03 §2.1).
pub const CHUNK_SIZE: u64 = 1024 * 1024;

/// Default logical sector size assumed when a source cannot report one.
pub const DEFAULT_SECTOR_SIZE: u32 = 512;
