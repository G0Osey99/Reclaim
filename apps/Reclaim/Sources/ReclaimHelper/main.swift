// The Reclaim privileged helper (doc 03 §7). Registered as an
// `SMAppService.daemon` LaunchDaemon; runs as root; exposes a tiny XPC API so
// the unprivileged app can obtain a read-only device fd (plus SMART and APFS
// unlock via `diskutil`). It opens fds and shells out to `diskutil` — it never
// parses on-disk data. All device reads remain O_RDONLY.

import Foundation
import ReclaimHelperProtocol

let helperVersion = "1.0.0"

// A peer closing its end of a pipe must not kill the daemon.
signal(SIGPIPE, SIG_IGN)

/// Run a tool and capture stdout; passphrase (if any) is fed on stdin.
@discardableResult
func run(_ launchPath: String, _ args: [String], stdin: String? = nil) -> (Int32, String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: launchPath)
    p.arguments = args
    let outPipe = Pipe()
    p.standardOutput = outPipe
    let errPipe = Pipe()
    p.standardError = errPipe
    if let s = stdin {
        let inPipe = Pipe()
        p.standardInput = inPipe
        do { try p.run() } catch { return (-1, "\(error)") }
        inPipe.fileHandleForWriting.write(Data(s.utf8))
        inPipe.fileHandleForWriting.closeFile()
    } else {
        do { try p.run() } catch { return (-1, "\(error)") }
    }
    let data = outPipe.fileHandleForReading.readDataToEndOfFile()
    // Drain stderr too so a chatty tool can never block on a full pipe.
    _ = errPipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return (p.terminationStatus, String(data: data, encoding: .utf8) ?? "")
}

final class HelperService: NSObject, ReclaimHelperXPC {
    func ping(reply: @escaping (String) -> Void) {
        reply("reclaim-helper \(helperVersion)")
    }

    func openDeviceReadOnly(bsdName: String, reply: @escaping (FileHandle?, String?) -> Void) {
        guard isValidBSDName(bsdName) else {
            reply(nil, "invalid device name")
            return
        }
        let bare = bsdName.hasPrefix("r") ? String(bsdName.dropFirst()) : bsdName
        let node = "/dev/r\(bare)"
        let fd = open(node, O_RDONLY | O_CLOEXEC | O_NOFOLLOW)
        if fd < 0 {
            reply(nil, "open \(node): \(String(cString: strerror(errno)))")
            return
        }
        // Only ever hand out a character device (the raw disk node).
        var st = stat()
        guard fstat(fd, &st) == 0, (st.st_mode & S_IFMT) == S_IFCHR else {
            close(fd)
            reply(nil, "\(node) is not a character device")
            return
        }
        // The FileHandle owns the fd; XPC dup's it into the app process.
        reply(FileHandle(fileDescriptor: fd, closeOnDealloc: true), nil)
    }

    func smart(bsdName: String, reply: @escaping (String?, String?) -> Void) {
        guard isValidBSDName(bsdName) else {
            reply(nil, "invalid device name")
            return
        }
        let (code, out) = run("/usr/sbin/diskutil", ["info", "-plist", bsdName])
        if code != 0 {
            reply(nil, "diskutil exited \(code)")
            return
        }
        // Parse only the SMARTStatus field (the helper does no on-disk parsing).
        var status = "NotAvailable"
        if let data = out.data(using: .utf8),
            let plist = try? PropertyListSerialization.propertyList(from: data, format: nil),
            let dict = plist as? [String: Any],
            let s = dict["SMARTStatus"] as? String
        {
            status = s
        }
        reply("{\"status\":\"\(status)\"}", nil)
    }

    func unlockAPFS(volume: String, passphrase: String, reply: @escaping (Bool, String?) -> Void) {
        guard isValidBSDName(volume) else {
            reply(false, "invalid volume name")
            return
        }
        let (code, out) = run(
            "/usr/sbin/diskutil",
            ["apfs", "unlockVolume", volume, "-nomount", "-stdinpassphrase"],
            stdin: passphrase
        )
        reply(code == 0, code == 0 ? nil : out)
    }
}

final class ListenerDelegate: NSObject, NSXPCListenerDelegate {
    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection c: NSXPCConnection) -> Bool {
        // Fail closed: pin the caller to our team *and* the app's bundle id
        // (doc 03 §7 "both ways"). With no team baked in (ad-hoc build) there is
        // nothing to pin against, so refuse rather than serve any caller as root.
        guard let team = reclaimConfiguredTeamID() else { return false }
        c.setCodeSigningRequirement(
            reclaimCodeRequirement(teamID: team, identifier: reclaimAppBundleIdentifier))
        c.exportedInterface = NSXPCInterface(with: ReclaimHelperXPC.self)
        c.exportedObject = HelperService()
        c.resume()
        return true
    }
}

let delegate = ListenerDelegate()
let listener = NSXPCListener(machServiceName: reclaimHelperMachServiceName)
listener.delegate = delegate
listener.resume()
dispatchMain()
