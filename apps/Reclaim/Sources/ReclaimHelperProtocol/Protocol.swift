import Foundation

/// The privileged helper's Mach service name (matches the LaunchDaemons plist
/// and the `SMAppService.daemon(plistName:)` registration).
public let reclaimHelperMachServiceName = "com.reclaim.helper"

/// The code-signing requirement each side pins on the XPC connection so the app
/// only talks to *its* helper and the helper only serves *its* app (doc 03 §7,
/// "Code requirement checks both ways"). For a Developer ID build set
/// `RECLAIM_TEAM_ID`; the ad-hoc dev build falls back to a same-team check that
/// is documented as not enforceable without a real team (see phase-5 log).
public func reclaimCodeRequirement(teamID: String?) -> String {
    if let team = teamID, !team.isEmpty {
        return "anchor apple generic and certificate leaf[subject.OU] = \"\(team)\""
    }
    // Ad-hoc / development: no team to pin. The caller decides whether to enforce.
    return "anchor apple generic"
}

/// The XPC surface the helper exposes (doc 08 §5 / doc 03 §7). Deliberately
/// tiny: open a device read-only and return its fd, report SMART, unlock an APFS
/// volume. The helper never parses on-disk data — the in-process core does.
@objc public protocol ReclaimHelperXPC {
    /// Liveness check; returns the helper's version string.
    func ping(reply: @escaping (String) -> Void)

    /// Open `/dev/r<bsdName>` `O_RDONLY` and hand back the file descriptor as a
    /// `FileHandle`. On failure, `error` is set and the handle is nil.
    func openDeviceReadOnly(bsdName: String, reply: @escaping (FileHandle?, String?) -> Void)

    /// The SMART status for a whole disk, as a JSON object string
    /// (`{"status":"Verified"|"Failing"|...}`), via `diskutil info -plist`.
    func smart(bsdName: String, reply: @escaping (String?, String?) -> Void)

    /// Unlock a FileVault/APFS volume via `diskutil apfs unlockVolume`
    /// (passphrase over stdin, never argv). Returns whether it unlocked.
    func unlockAPFS(volume: String, passphrase: String, reply: @escaping (Bool, String?) -> Void)
}
