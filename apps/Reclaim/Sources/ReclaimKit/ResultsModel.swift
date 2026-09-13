import Foundation
import ReclaimCore

/// Browses a session's results: paging, filtering, family counts, selection, and
/// preview loading (doc 08 §2.3/§4). Reads through a `Session` handle (its own
/// SQLite read connection), so it works during a live scan or on a reopened
/// session.
@MainActor
public final class ResultsModel: ObservableObject {
    public let session: Session

    @Published public var filter = FilterState()
    @Published public private(set) var page: [ResultRecord] = []
    @Published public private(set) var total: UInt64 = 0
    @Published public private(set) var families: [FamilyCount] = []
    @Published public var selection = Set<String>()
    @Published public var focused: String?
    @Published public var offset: UInt32 = 0
    @Published public var pageSize: UInt32 = 200
    @Published public private(set) var errorMessage: String?

    public init(session: Session) {
        self.session = session
    }

    /// The number of results currently selected.
    public var selectedCount: Int { selection.count }

    /// Total bytes of the current selection (from the loaded page; a full-set
    /// tally would need a query, this covers the visible selection footer).
    public var selectedBytes: UInt64 {
        page.filter { selection.contains($0.id) }.reduce(0) { $0 + $1.len }
    }

    /// Reload the current page and counts for the active filter.
    public func reload() {
        do {
            let f = filter.resultFilter()
            let p = try session.query(filter: f, offset: offset, limit: pageSize)
            self.page = p.items
            self.total = p.total
            self.families = try session.familyCounts(deletedOnly: filter.deletedOnly)
            self.errorMessage = nil
        } catch {
            self.errorMessage = "\(error)"
        }
    }

    /// Jump to a page (0-based page index) and reload.
    public func goToPage(_ index: Int) {
        offset = UInt32(max(0, index)) * pageSize
        reload()
    }

    public var pageCount: Int {
        guard pageSize > 0 else { return 1 }
        return max(1, Int((total + UInt64(pageSize) - 1) / UInt64(pageSize)))
    }

    public func toggle(_ id: String) {
        if selection.contains(id) { selection.remove(id) } else { selection.insert(id) }
    }

    public func selectAllOnPage() {
        for r in page { selection.insert(r.id) }
    }

    public func clearSelection() { selection.removeAll() }

    /// Render a preview (image thumbnail / full PNG / text / hex).
    public func preview(_ id: String, kind: PreviewKind, maxPx: UInt32 = 256) -> PreviewResult? {
        try? session.preview(id: id, kind: kind, maxPx: maxPx)
    }

    /// A recover destination check for the current selection.
    public func checkDestination(_ dest: String) -> DestinationCheck? {
        try? session.checkDestination(dest: dest, neededBytes: selectedBytes)
    }

    /// The currently focused record (for the inspector).
    public var focusedRecord: ResultRecord? {
        guard let id = focused else { return nil }
        return page.first { $0.id == id }
    }

    /// Recover the current selection, streaming per-file progress to `onEvent`.
    public func recover(options: RecoverOptions, sink: EventSink) throws -> RecoverOutcome {
        try session.recover(ids: Array(selection), opts: options, sink: sink)
    }
}
