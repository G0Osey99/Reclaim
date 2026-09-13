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

    /// Register the daemon, then branch on the resulting **status**, not on
    /// whether `register()` threw (doc 08 §2.1). Per Apple DTS (Forums 707482 /
    /// 799910): the normal daemon flow is `register()` returns and status becomes
    /// `.requiresApproval` — the user must then enable it in System Settings →
    /// General → Login Items & Extensions. A "code 1 / Operation not permitted"
    /// throw on an *already-registered* daemon is expected on a repeat press, so
    /// we ignore the throw and read `status`; a throw while status is still
    /// `.notRegistered`/`.notFound` is a genuine failure (usually a signing or
    /// bundle-location problem — the app must be signed with a real Apple cert
    /// and run from /Applications).
    public func install() {
        #if canImport(ServiceManagement)
        let service = SMAppService.daemon(plistName: Self.plistName)
        var registerError: Error?
        do {
            try service.register()
        } catch {
            registerError = error
        }
        refreshStatus()
        switch service.status {
        case .enabled:
            break
        case .requiresApproval:
            // Registered and waiting for the user — send them to the toggle.
            openLoginItems()
        default:
            let base = registerError.map { "\($0.localizedDescription)" } ?? "registration failed"
            status = .failed(
                "\(base). Make sure Reclaim.app is in /Applications and launched "
                    + "from there, then try again. If it persists, remove any stale "
                    + "registration (see the app's help) and relaunch.")
        }
        #else
        status = .failed("ServiceManagement unavailable")
        #endif
    }

    /// Open System Settings → General → Login Items & Extensions so the user can
    /// enable the daemon (there is no per-daemon deep link).
    public func openLoginItems() {
        #if canImport(ServiceManagement)
        SMAppService.openSystemSettingsLoginItems()
        #endif
    }

    /// Unregister the daemon (clears a stale registration).
    public func uninstall() {
        #if canImport(ServiceManagement)
        let service = SMAppService.daemon(plistName: Self.plistName)
        try? service.unregister()
        refreshStatus()
        #endif
    }

    /// Refresh `status` from `SMAppService`. Preserves a prior `.failed` only
    /// until the real status is known.
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

    /// Re-read the helper status and, when it is enabled, re-probe Full Disk
    /// Access. Called on a timer and whenever the app regains focus during
    /// onboarding — SMAppService has no status-change notification, so polling is
    /// the documented approach (Apple DTS, FB17671405).
    public func refresh(bootWholeDisk: String?) async {
        refreshStatus()
        if status == .enabled, !fullDiskAccess, let boot = bootWholeDisk {
            await probeFullDiskAccess(bootWholeDisk: boot)
        }
    }

    /// True once both gates are satisfied.
    public var isReady: Bool { status == .enabled && fullDiskAccess }

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
