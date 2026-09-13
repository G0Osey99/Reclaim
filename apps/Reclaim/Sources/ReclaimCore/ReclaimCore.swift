// ReclaimCore convenience surface. The generated `reclaim_ffi.swift` (same
// target) already declares the full API at module scope; this file adds a small
// version helper so callers can `import ReclaimCore`.
public enum ReclaimCoreInfo {
    /// The core library version (shared with the CLI).
    public static var version: String { ReclaimCore.version() }
}
