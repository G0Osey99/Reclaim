import Foundation
import ReclaimCore

/// The scan-plan model behind the "Scan options inspector" (GUI Option B, doc 08
/// §2.2). A **preset** (Quick / Deep / Full) is a canonical `ScanOptions`; the
/// moment any option is hand-edited the plan re-derives to **Custom** — and snaps
/// back to a named preset if the edits happen to reconstruct one. All of the
/// preset↔options logic lives here as pure value code so it is unit-tested
/// without a view (see `ReclaimSelfTest` / `XcodeTests/ScanPlanTests`).

// MARK: - Defaults & builder

/// The non-pass defaults shared by every preset — the values `Options.scan`
/// produces. "Custom" is defined as any deviation from these (plus the two pass
/// flags). Kept in one place so the builder, the detector and the presets agree.
public enum ScanDefaults {
    public static let families: [String] = []
    public static let sigIds: [String] = []
    public static let blockSize: UInt32 = 0            // 0 = auto
    public static let bruteForce = false
    public static let keepCorrupted = true
    public static let maxFileSize: UInt64 = 4 * 1024 * 1024 * 1024
    public static let unallocatedOnly = false
    public static let checkpointSecs: UInt64 = 5
}

/// Build a `ScanOptions` from the passes plus any overrides (`resume` is always
/// false here — resuming is a session action, not a plan setting).
public func makeScanOptions(
    quick: Bool,
    deep: Bool,
    families: [String] = ScanDefaults.families,
    blockSize: UInt32 = ScanDefaults.blockSize,
    bruteForce: Bool = ScanDefaults.bruteForce,
    keepCorrupted: Bool = ScanDefaults.keepCorrupted,
    maxFileSize: UInt64 = ScanDefaults.maxFileSize,
    rangeStart: UInt64? = nil,
    rangeEnd: UInt64? = nil,
    unallocatedOnly: Bool = ScanDefaults.unallocatedOnly,
    checkpointSecs: UInt64 = ScanDefaults.checkpointSecs
) -> ScanOptions {
    ScanOptions(
        quick: quick,
        deep: deep,
        families: families,
        sigIds: ScanDefaults.sigIds,
        blockSize: blockSize,
        bruteForce: bruteForce,
        keepCorrupted: keepCorrupted,
        maxFileSize: maxFileSize,
        rangeStart: rangeStart,
        rangeEnd: rangeEnd,
        unallocatedOnly: unallocatedOnly,
        checkpointSecs: checkpointSecs,
        resume: false)
}

// MARK: - Preset

public enum ScanPreset: String, CaseIterable, Identifiable, Sendable {
    case quick, deep, full, custom
    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .quick: return "Quick"
        case .deep: return "Deep"
        case .full: return "Full"
        case .custom: return "Custom"
        }
    }

    /// One-line description shown under the preset control.
    public var summary: String {
        switch self {
        case .quick: return "Named files and recent deletions from the filesystem."
        case .deep: return "Signature-carve free space for lost files."
        case .full: return "Named files, carving and lost-structure recovery."
        case .custom: return "Hand-tuned options."
        }
    }

    /// Presets shown in the segmented control (Custom is derived, never picked).
    public static let selectable: [ScanPreset] = [.quick, .deep, .full]

    /// The canonical options for a named preset (nil for `.custom`).
    public func canonicalOptions() -> ScanOptions? {
        switch self {
        case .quick: return makeScanOptions(quick: true, deep: false)
        case .deep: return makeScanOptions(quick: false, deep: true)
        case .full: return makeScanOptions(quick: true, deep: true)
        case .custom: return nil
        }
    }

    /// Which preset an options set matches, or `.custom`. Any non-pass deviation
    /// from `ScanDefaults` is Custom; otherwise the (quick, deep) pair names it.
    public static func detect(from o: ScanOptions) -> ScanPreset {
        guard o.families.isEmpty,
            o.sigIds.isEmpty,
            o.blockSize == ScanDefaults.blockSize,
            o.bruteForce == ScanDefaults.bruteForce,
            o.keepCorrupted == ScanDefaults.keepCorrupted,
            o.maxFileSize == ScanDefaults.maxFileSize,
            o.rangeStart == nil,
            o.rangeEnd == nil,
            o.unallocatedOnly == ScanDefaults.unallocatedOnly,
            o.checkpointSecs == ScanDefaults.checkpointSecs
        else { return .custom }
        switch (o.quick, o.deep) {
        case (true, false): return .quick
        case (false, true): return .deep
        case (true, true): return .full
        case (false, false): return .custom   // nothing to run
        }
    }
}

// MARK: - Family groups (carve restriction)

/// A user-facing family group in the inspector, mapped to the core family strings
/// the carve engine accepts via `ScanOptions.families` (empty = all).
public struct ScanFamilyGroup: Identifiable, Hashable, Sendable {
    public let id: String
    public let label: String
    public let symbol: String        // SF Symbol
    public let cores: [String]       // core family strings

    public static let all: [ScanFamilyGroup] = [
        .init(id: "photos", label: "Photos", symbol: "photo", cores: ["image", "raw"]),
        .init(id: "video", label: "Video", symbol: "film", cores: ["video"]),
        .init(id: "audio", label: "Audio", symbol: "music.note", cores: ["audio"]),
        .init(id: "docs", label: "Documents", symbol: "doc.text", cores: ["doc"]),
        .init(id: "archives", label: "Archives", symbol: "archivebox", cores: ["archive"]),
        .init(
            id: "other", label: "Other", symbol: "square.dashed",
            cores: ["exec", "model", "disk", "db", "crypto", "misc", "sys", "mail", "geo", "font", "app"]),
    ]
}

public enum ScanFamilies {
    /// The groups represented by a `families` array. Empty array (all) → every group.
    public static func selectedGroups(from families: [String]) -> Set<String> {
        if families.isEmpty { return Set(ScanFamilyGroup.all.map(\.id)) }
        let set = Set(families)
        return Set(
            ScanFamilyGroup.all
                .filter { g in g.cores.contains { set.contains($0) } }
                .map(\.id))
    }

    /// The `families` array for a set of selected group ids. All selected → []
    /// (the core's "all families") so a full selection round-trips to a preset.
    public static func families(for groups: Set<String>) -> [String] {
        if groups.count >= ScanFamilyGroup.all.count { return [] }
        return ScanFamilyGroup.all.filter { groups.contains($0.id) }.flatMap(\.cores)
    }
}

// MARK: - Discrete option tables (pickers)

public enum ScanBlockSize {
    public static let options: [(label: String, value: UInt32)] = [
        ("Auto", 0), ("512 B", 512), ("4 KB", 4096), ("64 KB", 65_536), ("1 MB", 1_048_576),
    ]
    public static func label(_ v: UInt32) -> String {
        options.first { $0.value == v }?.label ?? "\(v) B"
    }
}

public enum ScanMaxFileSize {
    public static let options: [(label: String, value: UInt64)] = [
        ("256 MB", 256 << 20), ("1 GB", 1 << 30), ("4 GB", UInt64(4) << 30),
        ("16 GB", UInt64(16) << 30), ("64 GB", UInt64(64) << 30),
    ]
    public static func label(_ v: UInt64) -> String {
        options.first { $0.value == v }?.label ?? formatBytes(v)
    }
}

public enum ScanCheckpoint {
    public static let options: [(label: String, value: UInt64)] = [
        ("Every 5 s", 5), ("Every 15 s", 15), ("Every 30 s", 30), ("Every 60 s", 60),
    ]
    public static func label(_ v: UInt64) -> String {
        options.first { $0.value == v }?.label ?? "Every \(v) s"
    }
}

// MARK: - Estimate

/// Format a duration (seconds) compactly for the estimate line.
public func formatDuration(_ seconds: Double) -> String {
    guard seconds.isFinite, seconds > 0 else { return "—" }
    let s = Int(seconds.rounded())
    if s < 90 { return "\(max(1, s)) s" }
    let m = Int((seconds / 60).rounded())
    if m < 90 { return "\(m) min" }
    return String(format: "%.1f h", seconds / 3600)
}

/// A rough scan-time estimate from a full-source read at the probed speed. The
/// carving passes (Deep/Full) read the whole source once; Quick reads only
/// filesystem metadata, so its time is dominated by seeks, not throughput —
/// reported as a short metadata pass rather than a fabricated number.
public func estimatedScanText(preset: ScanPreset, sizeBytes: UInt64, readBps: Double) -> String? {
    guard readBps > 0, sizeBytes > 0 else { return nil }
    let fullRead = Double(sizeBytes) / readBps
    switch preset {
    case .quick:
        return "metadata pass — usually under a minute"
    case .deep, .full:
        return "≈ \(formatDuration(fullRead)) (one full read at \(formatBytes(UInt64(readBps)))/s)"
    case .custom:
        return "≈ \(formatDuration(fullRead)) if it reads the whole source"
    }
}

// MARK: - Model

/// Observable scan-plan state for the source-detail screen. `options` is the
/// source of truth; `preset` is derived. `select(_:)` applies a preset; `edit`
/// mutates options and re-derives the preset (this is what makes "touch any
/// option → Custom" work, and snaps back to a preset when edits reconstruct one).
@MainActor
public final class ScanPlanModel: ObservableObject {
    @Published public private(set) var options: ScanOptions
    @Published public private(set) var preset: ScanPreset

    public init(preset: ScanPreset = .full) {
        let start = preset == .custom ? ScanPreset.full : preset
        self.preset = start
        self.options = start.canonicalOptions() ?? makeScanOptions(quick: true, deep: true)
    }

    /// Apply a named preset, resetting options to its canonical set.
    public func select(_ preset: ScanPreset) {
        guard let opts = preset.canonicalOptions() else { return }
        self.options = opts
        self.preset = preset
    }

    /// Reset the plan to the default (Full) preset.
    public func reset() { select(.full) }

    /// Mutate options and re-derive the preset.
    public func edit(_ mutate: (inout ScanOptions) -> Void) {
        var o = options
        mutate(&o)
        options = o
        preset = ScanPreset.detect(from: o)
    }

    // Family-group selection helpers for the inspector.

    public var selectedFamilyGroups: Set<String> {
        ScanFamilies.selectedGroups(from: options.families)
    }

    public func isFamilyGroupOn(_ id: String) -> Bool {
        selectedFamilyGroups.contains(id)
    }

    /// Toggle a family group. At least one group must stay selected (an empty
    /// carve family list would mean "all" in the core, which is the opposite of
    /// what deselecting everything implies), so the last group cannot be removed.
    public func toggleFamilyGroup(_ id: String) {
        var groups = selectedFamilyGroups
        if groups.contains(id) {
            guard groups.count > 1 else { return }
            groups.remove(id)
        } else {
            groups.insert(id)
        }
        edit { $0.families = ScanFamilies.families(for: groups) }
    }
}
