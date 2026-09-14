import Foundation
import ReclaimCore

/// Drives a scan and surfaces live progress to SwiftUI (doc 08 §2.3). Holds the
/// `Session` so results can be paged from SQLite *while the scan writes* — the
/// browsing methods open their own read connection (doc 08 §4).
@MainActor
public final class ScanController: ObservableObject {
    @Published public private(set) var isScanning = false
    @Published public private(set) var paused = false
    @Published public private(set) var pct: Double = 0
    @Published public private(set) var rate: UInt64 = 0
    @Published public private(set) var pass: String = ""
    @Published public private(set) var foundCount: Int = 0
    @Published public private(set) var readErrors: Int = 0
    @Published public private(set) var volumeProposals: Int = 0
    @Published public private(set) var lastCheckpoint: Date? = nil
    @Published public private(set) var summary: ScanSummary?
    @Published public private(set) var errorMessage: String?
    @Published public private(set) var session: Session?

    /// Family counters accumulated from `Found` events (the live "found by
    /// family" strip, doc 08 §2.3), keyed by top-level family from the path.
    @Published public private(set) var foundByFamily: [String: Int] = [:]

    private var forwarder: EventForwarder?
    private var lastOpts: ScanOptions?
    private var lastSource: String?
    /// The helper fd behind an `fd:N:<bsd>` source. The session re-parses that
    /// spec (and re-dups the fd) for preview/recover, so it must outlive the
    /// session, not just the `startScan` call.
    private var device: DeviceFD?

    public init() {}

    deinit { device?.close() }

    /// Start a scan of `source` into `sessionDir` (nil = a default dir). Pass the
    /// `DeviceFD` behind an `fd:` spec as `device`; the controller owns it for
    /// the session's lifetime.
    public func start(source: String, device: DeviceFD? = nil, sessionDir: String?, opts: ScanOptions) {
        reset()
        session = nil
        self.device?.close()
        self.device = device
        lastOpts = opts
        lastSource = source
        let fwd = EventForwarder { [weak self] ev in self?.handle(ev) }
        forwarder = fwd
        do {
            session = try startScan(source: source, sessionDir: sessionDir, opts: opts, sink: fwd)
            isScanning = true
        } catch {
            errorMessage = "\(error)"
            self.device?.close()
            self.device = nil
        }
    }

    public func pause() {
        session?.pause()
        paused = true
    }

    public func resume() {
        guard let session, let opts = lastOpts else { return }
        let fwd = EventForwarder { [weak self] ev in self?.handle(ev) }
        forwarder = fwd
        do {
            try session.resumeScan(opts: opts, sink: fwd)
            paused = false
            isScanning = true
        } catch {
            errorMessage = "\(error)"
        }
    }

    public func stop() {
        session?.stop()
    }

    private func reset() {
        pct = 0; rate = 0; pass = ""; foundCount = 0; readErrors = 0
        volumeProposals = 0; summary = nil; errorMessage = nil
        foundByFamily = [:]; paused = false
    }

    private func handle(_ ev: RcEvent) {
        switch ev {
        case let .progress(pass, _, pct, rate):
            self.pass = pass
            self.pct = pct
            self.rate = rate
            self.lastCheckpoint = Date()
        case let .found(_, _, path, _, _):
            foundCount += 1
            let fam = path.split(separator: "/").first.map(String.init) ?? "other"
            foundByFamily[fam, default: 0] += 1
        case .volume:
            volumeProposals += 1
        case .readError:
            readErrors += 1
        case let .warning(msg):
            if msg.lowercased().contains("panic") || msg.lowercased().contains("stopped") {
                errorMessage = msg
            }
        case .passComplete:
            break
        case let .done(found, elapsed, interrupted):
            isScanning = false
            var total = found
            if let s = session, let c = try? s.count() { total = c }
            summary = ScanSummary(
                found: found, interrupted: interrupted, elapsed: elapsed,
                complete: !interrupted, vanished: false, totalRows: total)
        case .imageProgress, .recoverProgress:
            break
        }
    }
}
