//! Exit codes (docs/plan/07 §3) and the command error type.

/// Reclaim CLI exit codes.
///
/// The full contract is defined now (docs/plan/07 §3); the warning/interrupt/
/// refuse codes are produced by the scan/recover commands that land in later
/// phases, so they are allowed to be unused in Phase 0.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Exit {
    /// Success.
    Success = 0,
    /// Usage error.
    Usage = 1,
    /// Permission / root / Full Disk Access problem (`doctor` explains).
    Permission = 2,
    /// Completed with warnings (bad sectors, partial files).
    Warnings = 3,
    /// Interrupted, session resumable.
    Interrupted = 4,
    /// Refused for safety (same-device destination, writable mount).
    Refused = 5,
    /// Source not found or vanished mid-scan.
    NotFound = 6,
    /// Internal error (bug).
    Internal = 7,
}

/// A command failure carrying an exit code and a human message.
#[derive(Debug)]
pub struct CmdError {
    /// Exit code to return.
    pub code: Exit,
    /// Human-readable message printed to stderr.
    pub message: String,
}

impl CmdError {
    /// Build a command error.
    pub fn new(code: Exit, message: impl Into<String>) -> Self {
        CmdError {
            code,
            message: message.into(),
        }
    }

    /// Source-not-found error (exit 6).
    pub fn not_found(message: impl Into<String>) -> Self {
        CmdError::new(Exit::NotFound, message)
    }

    /// Permission error (exit 2).
    pub fn permission(message: impl Into<String>) -> Self {
        CmdError::new(Exit::Permission, message)
    }

    /// Internal error (exit 7).
    pub fn internal(message: impl Into<String>) -> Self {
        CmdError::new(Exit::Internal, message)
    }
}

/// Convenience result alias for command handlers.
pub type CmdResult = Result<Exit, CmdError>;
