//! UniFFI bindings over the Reclaim core (docs/plan/08 §5, §3.7).
//!
//! This crate is the **only** new "logic" the GUI needs, and even that is thin:
//! it resolves a source string, drives the same scan / query / recover / image /
//! volumes / snapshots / report paths the CLI uses (from `reclaim-session`,
//! `reclaim-block`, `reclaim-part`, `reclaim-structs`, `reclaim-report`,
//! `reclaim-platform-macos`, `fs-apfs`), and marshals the results across the FFI
//! for Swift. It writes nothing itself: recovery and preview-extraction go
//! through `reclaim_session::recover` (the allow-listed writer, build guide
//! Part 3.2), so `check-readonly` finds no write pattern in this crate.
//!
//! The public surface mirrors doc 08 §5: `list_sources`, `probe`,
//! `open_session`, `start_scan` + `EventSink`, `pause`/`resume`/`stop`, `query`
//! (paged from SQLite), `preview`, `recover`, `image`, `volumes`, `snapshots`,
//! `report`.

#![forbid(unsafe_op_in_unsafe_fn)]

mod enumerate;
mod ffitypes;
mod imagejob;
mod resolve;
mod session;
mod snapshots;
mod volumes;

pub use enumerate::{list_sources, probe};
pub use ffitypes::*;
pub use imagejob::{start_image, ImageJob};
pub use session::{open_session, start_scan, Session};
pub use snapshots::{snapshot_diff, snapshots};
pub use volumes::volumes;

uniffi::setup_scaffolding!();

/// A sink the GUI implements to receive live scan / recover / image events
/// (doc 08 §5 `callback interface EventSink`). Implementations must be safe to
/// call from a background thread (the Swift side hops to the main actor for UI).
#[uniffi::export(callback_interface)]
pub trait EventSink: Send + Sync {
    /// Called for each event as work progresses.
    fn on_event(&self, event: RcEvent);
}

/// The library version — the same workspace version the CLI reports (doc 08 §6).
#[uniffi::export]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Errors surfaced to Swift. Each carries a human message; the variant lets the
/// UI branch (e.g. a permission failure routes to the helper/FDA onboarding,
/// a refusal shows inline red text — doc 08 §3).
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum RcError {
    /// The source/session/result could not be found.
    #[error("{msg}")]
    NotFound {
        /// Human-readable detail.
        msg: String,
    },
    /// Reading the source needs privileges the process does not have (root /
    /// Full Disk Access) — the GUI routes this to onboarding.
    #[error("{msg}")]
    Permission {
        /// Human-readable detail.
        msg: String,
    },
    /// A safety refusal (e.g. same-disk destination).
    #[error("{msg}")]
    Refused {
        /// Human-readable detail.
        msg: String,
    },
    /// Invalid arguments / usage.
    #[error("{msg}")]
    Usage {
        /// Human-readable detail.
        msg: String,
    },
    /// An I/O or internal error.
    #[error("{msg}")]
    Internal {
        /// Human-readable detail.
        msg: String,
    },
}

impl RcError {
    pub(crate) fn not_found(m: impl Into<String>) -> Self {
        RcError::NotFound { msg: m.into() }
    }
    pub(crate) fn permission(m: impl Into<String>) -> Self {
        RcError::Permission { msg: m.into() }
    }
    pub(crate) fn refused(m: impl Into<String>) -> Self {
        RcError::Refused { msg: m.into() }
    }
    pub(crate) fn usage(m: impl Into<String>) -> Self {
        RcError::Usage { msg: m.into() }
    }
    pub(crate) fn internal(m: impl Into<String>) -> Self {
        RcError::Internal { msg: m.into() }
    }
}

impl From<reclaim_session::SessionError> for RcError {
    fn from(e: reclaim_session::SessionError) -> Self {
        use reclaim_session::SessionError as S;
        match e {
            S::SourceMismatch(m) => RcError::Refused { msg: m },
            other => RcError::Internal {
                msg: other.to_string(),
            },
        }
    }
}
