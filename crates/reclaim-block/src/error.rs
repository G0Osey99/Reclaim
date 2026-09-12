//! Error type for the block layer.

/// Errors from opening or configuring a [`crate::BlockSource`].
///
/// Note that a *read* never returns an `Err`: [`crate::BlockSource::read_at`]
/// reports bad/unread sectors inside its [`crate::ReadResult`] so callers can
/// keep going (docs/plan/03 §6 — "a read error is a value, not an exception").
/// This type is for construction-time failures (open, stat, geometry).
#[derive(Debug, thiserror::Error)]
pub enum BlockError {
    /// The underlying open/stat/geometry syscall failed.
    #[error("i/o error on {path}: {source}")]
    Io {
        /// Path that was being opened or queried.
        path: String,
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// A source was asked for an offset at or beyond its length.
    #[error("offset {offset} is beyond source length {len}")]
    OutOfRange {
        /// Requested byte offset.
        offset: u64,
        /// Source length in bytes.
        len: u64,
    },

    /// A requested window does not fit inside the parent source.
    #[error("window [{start}, {start}+{len}) does not fit in source of length {source_len}")]
    WindowOutOfRange {
        /// Window start byte offset within the parent.
        start: u64,
        /// Window length in bytes.
        len: u64,
        /// Parent source length in bytes.
        source_len: u64,
    },

    /// A geometry value (sector size) was zero or otherwise invalid.
    #[error("invalid geometry: {0}")]
    Geometry(String),

    /// A recognized image container had malformed or unsupported metadata
    /// (docs/plan/04 §5). The magic matched but the structure could not be
    /// parsed into a usable block map.
    #[error("container format error: {0}")]
    Container(String),
}

impl BlockError {
    pub(crate) fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        BlockError::Io {
            path: path.into(),
            source,
        }
    }
}
