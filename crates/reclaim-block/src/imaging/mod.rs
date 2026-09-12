//! Imaging (docs/plan/02 FR-IMG): a multi-pass, resumable byte-for-byte imager
//! with a bad-block map, optional sparse/zstd output, per-chunk and whole-image
//! hashing, and a [`MappedImage`] source that reads the result back (returning
//! `Unread` for never-imaged regions).
//!
//! This is the only part of `reclaim-block` that writes: [`dest::ImageDest`] is
//! allow-listed in `scripts/readonly-allowlist.txt`.

pub mod dest;
pub mod hash;
pub mod imager;
pub mod map;
pub mod mapped;

pub use imager::{image, verify_image, ImageOptions, ImageOutcome, ImageProgress, VerifyReport};
pub use map::{Compression, HashAlgo, ImageMap};
pub use mapped::MappedImage;
