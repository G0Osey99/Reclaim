import Foundation
import ReclaimCore

/// Recoverability bucket shown in the filter rail (doc 08 §2.3).
public enum Recoverability: String, CaseIterable, Sendable {
    case any, high, medium, low
    /// The minimum score this bucket maps to (the "chances" column).
    public var minScore: UInt8? {
        switch self {
        case .any: return nil
        case .high: return 80
        case .medium: return 50
        case .low: return 1
        }
    }
    public var label: String {
        switch self {
        case .any: return "Any"
        case .high: return "High"
        case .medium: return "Medium"
        case .low: return "Low"
        }
    }
}

/// The producing-engine facet (doc 08 §2.3 "Named files / Reconstructed / Carved").
public enum SourceKindFilter: String, CaseIterable, Sendable {
    case any, carved, named
    public var label: String {
        switch self {
        case .any: return "All engines"
        case .carved: return "Carved"
        case .named: return "Named / reconstructed"
        }
    }
}

/// Sort options exposed in the UI, mapped to the core `SortKey`.
public enum ResultSort: String, CaseIterable, Sendable {
    case offset, size, date, score, path
    public var core: SortKey {
        switch self {
        case .offset: return .offset
        case .size: return .size
        case .date: return .date
        case .score: return .score
        case .path: return .path
        }
    }
    public var label: String { rawValue.capitalized }
}

/// The GUI filter-rail state. `resultFilter()` is the single place UI state
/// becomes a core query (doc 08 §2.3) — the unit-tested "filters → SQL" mapping.
public struct FilterState: Equatable, Sendable {
    public var family: String?
    public var exts: [String]
    public var minSizeBytes: UInt64?
    public var afterDate: String?          // YYYY-MM-DD
    public var recoverability: Recoverability
    public var sourceKind: SourceKindFilter
    public var deletedOnly: Bool
    public var fullOnly: Bool
    public var includeMerged: Bool
    public var pathSearch: String?
    public var sort: ResultSort

    public init(
        family: String? = nil,
        exts: [String] = [],
        minSizeBytes: UInt64? = nil,
        afterDate: String? = nil,
        recoverability: Recoverability = .any,
        sourceKind: SourceKindFilter = .any,
        deletedOnly: Bool = false,
        fullOnly: Bool = false,
        includeMerged: Bool = false,
        pathSearch: String? = nil,
        sort: ResultSort = .offset
    ) {
        self.family = family
        self.exts = exts
        self.minSizeBytes = minSizeBytes
        self.afterDate = afterDate
        self.recoverability = recoverability
        self.sourceKind = sourceKind
        self.deletedOnly = deletedOnly
        self.fullOnly = fullOnly
        self.includeMerged = includeMerged
        self.pathSearch = pathSearch
        self.sort = sort
    }

    /// Build the core `ResultFilter`. A blank/whitespace path search maps to no
    /// glob; a search without any wildcard is wrapped as a `*substr*` contains.
    public func resultFilter() -> ResultFilter {
        let glob: String?
        if let raw = pathSearch?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty {
            glob = raw.contains("*") || raw.contains("?") ? raw : "*\(raw)*"
        } else {
            glob = nil
        }
        let engine: String? = sourceKind == .carved ? "carve" : nil
        return ResultFilter(
            family: family,
            exts: exts,
            minSize: minSizeBytes,
            after: afterDate,
            minScore: recoverability.minScore,
            engine: engine,
            fullOnly: fullOnly,
            deletedOnly: deletedOnly,
            includeMerged: includeMerged,
            pathGlob: glob,
            sort: sort.core
        )
    }
}

/// The verdict for a recover destination (doc 08 §2.4). Pure logic over a core
/// `DestinationCheck` so it is unit-tested without touching a disk.
public struct DestinationVerdict: Equatable, Sendable {
    /// Whether the Recover button should be enabled.
    public var canRecover: Bool
    /// A blocking reason (red inline text), if any.
    public var blockingReason: String?
    /// A non-blocking warning (e.g. low space), if any.
    public var warning: String?
}

/// Decide whether a recover may proceed. Same-disk is blocking unless the user
/// checked the override; low space is only a warning (docs/plan/06 §10).
public func evaluateDestination(
    _ check: DestinationCheck,
    allowSameDevice: Bool
) -> DestinationVerdict {
    if check.sameDisk && !allowSameDevice {
        return DestinationVerdict(
            canRecover: false,
            blockingReason: check.reason
                ?? "Destination is on the same physical disk as the source.",
            warning: nil
        )
    }
    let warning = check.lowSpace
        ? "Free space is low for the selected size (need ~10% headroom)."
        : nil
    return DestinationVerdict(canRecover: true, blockingReason: nil, warning: warning)
}

/// Human byte formatting shared by the views.
public func formatBytes(_ n: UInt64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB", "PB"]
    var v = Double(n)
    var i = 0
    while v >= 1024 && i < units.count - 1 {
        v /= 1024
        i += 1
    }
    return i == 0 ? "\(n) B" : String(format: "%.1f %@", v, units[i])
}
