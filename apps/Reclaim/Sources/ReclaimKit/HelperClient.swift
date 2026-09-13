import Foundation
import ReclaimHelperProtocol
#if canImport(ServiceManagement)
import ServiceManagement
#endif

/// Lifecycle + connection to the privileged helper (doc 08 §2.1, §3). Registers
/// the `SMAppService.daemon`, opens an XPC connection, obtains read-only device
/// fds, and probes Full Disk Access by attempting a boot-store raw read through
/// the helper.
@MainActor
public final class HelperClient: ObservableObject {
    /// The LaunchDaemons plist basename embedded in the app bundle.
    public static let plistName = "com.reclaim.helper.plist"

    public enum Status: Equatable {
        case notInstalled
        case requiresApproval
        case enabled
        case failed(String)
    }

    @Published public private(set) var status: Status = .notInstalled
    @Published public private(set) var fullDiskAccess: Bool = false

    public init() {}

    // MARK: registration

    /// Register the daemon (prompts the user via SMAppService, doc 08 §2.1).
    public func install() {
        #if canImport(ServiceManagement)
        let service = SMAppService.daemon(plistName: Self.plistName)
        do {
            try service.register()
            refreshStatus()
        } catch {
            status = .failed("\(error.localizedDescription)")
        }
        #else
        status = .failed("ServiceManagement unavailable")
        #endif
    }

    /// Unregister the daemon.
    public func uninstall() {
        #if canImport(ServiceManagement)
        let service = SMAppService.daemon(plistName: Self.plistName)
        try? service.unregister()
        refreshStatus()
        #endif
    }

    /// Refresh `status` from `SMAppService`.
    public func refreshStatus() {
        #if canImport(ServiceManagement)
        let service = SMAppService.daemon(plistName: Self.plistName)
        switch service.status {
        case .enabled: status = .enabled
        case .requiresApproval: status = .requiresApproval
        case .notRegistered, .notFound: status = .notInstalled
        @unknown default: status = .notInstalled
        }
        #endif
    }

    // MARK: XPC

    /// Pin the connection's code-signing requirement (both-ways check, doc 03 §7).
    @available(macOS 13.0, *)
    private func setRequirement(_ c: NSXPCConnection, _ req: String) {
        c.setCodeSigningRequirement(req)
    }

    private func connection() -> NSXPCConnection {
        let c = NSXPCConnection(machServiceName: reclaimHelperMachServiceName, options: .privileged)
        c.remoteObjectInterface = NSXPCInterface(with: ReclaimHelperXPC.self)
        if let team = reclaimConfiguredTeamID() {
            if #available(macOS 13.0, *) {
                setRequirement(c, reclaimCodeRequirement(teamID: team))
            }
        }
        c.resume()
        return c
    }

    private func proxy(_ c: NSXPCConnection, _ onError: @escaping (Error) -> Void) -> ReclaimHelperXPC? {
        c.remoteObjectProxyWithErrorHandler { err in onError(err) } as? ReclaimHelperXPC
    }

    /// Ping the helper; returns its version string, or throws.
    public func ping() async throws -> String {
        try await withCheckedThrowingContinuation { cont in
            let c = connection()
            guard let p = proxy(c, { cont.resume(throwing: $0) }) else {
                cont.resume(throwing: HelperError.noProxy)
                return
            }
            p.ping { version in
                c.invalidate()
                cont.resume(returning: version)
            }
        }
    }

    /// Open `bsd` read-only through the helper and return the fd as an Int32
    /// (the caller passes `fd:<n>:<bsd>` to the core). The returned `FileHandle`
    /// must be retained for the fd's lifetime — held by the returned box.
    public func openDevice(bsd: String) async throws -> DeviceFD {
        try await withCheckedThrowingContinuation { cont in
            let c = connection()
            guard let p = proxy(c, { cont.resume(throwing: $0) }) else {
                cont.resume(throwing: HelperError.noProxy)
                return
            }
            p.openDeviceReadOnly(bsdName: bsd) { handle, err in
                if let handle {
                    cont.resume(returning: DeviceFD(handle: handle, connection: c, bsd: bsd))
                } else {
                    c.invalidate()
                    cont.resume(throwing: HelperError.open(err ?? "unknown"))
                }
            }
        }
    }

    /// Probe Full Disk Access: attempt a boot-store raw read through the helper
    /// (doc 08 §2.1). `whole` is the boot whole-disk BSD (e.g. `disk0`/`disk3`).
    /// Success ⇒ FDA is effective; EPERM ⇒ FDA missing (docs/plan/06 §2).
    public func probeFullDiskAccess(bootWholeDisk: String) async {
        do {
            let dev = try await openDevice(bsd: bootWholeDisk)
            defer { dev.close() }
            let data = try dev.handle.read(upToCount: 512)
            fullDiskAccess = (data?.isEmpty == false)
        } catch {
            fullDiskAccess = false
        }
    }

    /// Open the Privacy & Security → Full Disk Access settings pane.
    public func openFullDiskAccessSettings() {
        let url = URL(
            string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!
        #if canImport(AppKit)
        NSWorkspace.shared.open(url)
        #endif
    }

    public enum HelperError: Error, LocalizedError {
        case noProxy
        case open(String)
        public var errorDescription: String? {
            switch self {
            case .noProxy: return "could not reach the helper"
            case .open(let m): return "helper could not open device: \(m)"
            }
        }
    }
}

#if canImport(AppKit)
import AppKit
#endif

/// A helper-provided device fd bundled with the XPC connection that owns it.
public final class DeviceFD {
    public let handle: FileHandle
    private let connection: NSXPCConnection
    public let bsd: String
    init(handle: FileHandle, connection: NSXPCConnection, bsd: String) {
        self.handle = handle
        self.connection = connection
        self.bsd = bsd
    }
    /// The `fd:<n>:<bsd>` source spec the core resolves (via `RawDevice::from_fd`).
    public var sourceSpec: String { "fd:\(handle.fileDescriptor):\(bsd)" }
    public func close() { connection.invalidate() }
}
