import Foundation
import ReclaimCore

/// A recent session on disk (doc 08 §1 sidebar "Sessions").
public struct SessionRef: Identifiable, Hashable, Sendable {
    public let id: String        // directory path
    public let name: String
    public let modified: Date
    public var dir: String { id }
}

/// Top-level app state (doc 08 §1): sources, the selected source + probe, recent
/// sessions, and the helper/FDA gates. Owns the `HelperClient`.
@MainActor
public final class AppModel: ObservableObject {
    @Published public private(set) var sources: [SourceSummary] = []
    @Published public var selectedSourceBSD: String?
    @Published public private(set) var probe: SourceProbe?
    @Published public private(set) var probing = false
    @Published public private(set) var sessions: [SessionRef] = []
    @Published public var errorMessage: String?
    @Published public private(set) var coreVersion: String = version()

    public let helper = HelperClient()

    public init() {}

    /// Enumerate sources (doc 08 §1). On non-macOS this throws; the UI shows it.
    /// Enumeration talks to IOKit/DiskArbitration, so it runs off the main
    /// thread and the result is applied back on the main actor.
    public func refreshSources() {
        Task { [weak self] in
            let result: Result<[SourceSummary], Error> = await Task.detached {
                do { return .success(try listSources()) } catch { return .failure(error) }
            }.value
            guard let self else { return }
            switch result {
            case .success(let s): self.sources = s
            case .failure(let e): self.errorMessage = "Could not list sources: \(e)"
            }
        }
        helper.refreshStatus()
        refreshSessions()
    }

    public var selectedSource: SourceSummary? {
        guard let bsd = selectedSourceBSD else { return nil }
        return sources.first { $0.bsdName == bsd }
    }

    /// Probe the selected source (health, geometry, partitions — doc 08 §2.2).
    ///
    /// The app is unprivileged, so opening `/dev/rdiskN` by name needs root and
    /// fails in the shipping app — which is why the plan used to fall back to a
    /// "Probe source" button that did nothing. So, exactly like a scan, prefer a
    /// read-only fd from the privileged helper (docs/plan/03 §7). The fd is kept
    /// alive until the core has dup'd it (i.e. `probe` returns), then closed.
    public func probeSelected() {
        guard let src = selectedSource else { return }
        probing = true
        probe = nil
        let bsd = src.bsdName
        Task { [weak self] in
            guard let self else { return }
            var dev: DeviceFD?
            if self.helper.status == .enabled {
                dev = try? await self.helper.openDevice(bsd: bsd)
            }
            let spec = dev?.sourceSpec ?? bsd
            let result: Result<SourceProbe, Error>
            do {
                let p = try await Task.detached { try ReclaimCore.probe(source: spec) }.value
                result = .success(p)
            } catch {
                result = .failure(error)
            }
            dev?.close()
            // Ignore a probe whose source is no longer selected (rapid switching);
            // the current selection's own probe governs the UI.
            guard self.selectedSourceBSD == bsd else { return }
            self.applyProbe(result)
        }
    }

    private func applyProbe(_ result: Result<SourceProbe, Error>) {
        probing = false
        switch result {
        case .success(let p): probe = p
        case .failure(let e): errorMessage = "Probe failed: \(e)"
        }
    }

    /// The whole-disk BSD name of the running system's boot disk, for the FDA
    /// probe (best-effort: the disk mounted at `/` or the Data volume).
    public var bootWholeDisk: String? {
        sources.first { $0.mountPoint == "/" || $0.mountPoint == "/System/Volumes/Data" }?
            .wholeDisk
    }

    /// Scan the sessions directory for reopenable sessions.
    public func refreshSessions() {
        let base = (NSHomeDirectory() as NSString)
            .appendingPathComponent("Library/Application Support/Reclaim/sessions")
        var refs: [SessionRef] = []
        let fm = FileManager.default
        if let entries = try? fm.contentsOfDirectory(atPath: base) {
            for e in entries {
                let dir = (base as NSString).appendingPathComponent(e)
                let db = (dir as NSString).appendingPathComponent("session.sqlite")
                guard fm.fileExists(atPath: db) else { continue }
                let attrs = try? fm.attributesOfItem(atPath: db)
                let date = (attrs?[.modificationDate] as? Date) ?? .distantPast
                refs.append(SessionRef(id: dir, name: e, modified: date))
            }
        }
        sessions = refs.sorted { $0.modified > $1.modified }
    }

    /// Open a session for browsing.
    public func openSession(_ ref: SessionRef) -> Session? {
        do { return try ReclaimCore.openSession(dir: ref.dir) } catch {
            errorMessage = "Open session failed: \(error)"
            return nil
        }
    }
}
