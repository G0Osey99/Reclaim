import Foundation

/// The privileged helper's Mach service name (matches the LaunchDaemons plist
/// and the `SMAppService.daemon(plistName:)` registration).
public let reclaimHelperMachServiceName = "com.reclaim.helper"

/// The app's bundle identifier (packaging/Info.plist `CFBundleIdentifier`); the
/// helper pins incoming connections to it.
public let reclaimAppBundleIdentifier = "com.reclaim.app"

/// The helper's code-signing identifier. It is a bare executable signed without
/// `-i` (scripts/build-app.sh), so codesign derives the identifier from the file
/// name `Contents/MacOS/ReclaimHelper`; the app pins its connection to it.
public let reclaimHelperCodeIdentifier = "ReclaimHelper"

/// The code-signing requirement each side pins on the XPC connection so the app
/// only talks to *its* helper and the helper only serves *its* app (doc 03 §7,
/// "Code requirement checks both ways"). The team ID is the cert's OU — the same
/// value for a free "Apple Development" personal team and a paid Developer ID.
public func reclaimCodeRequirement(teamID: String?) -> String {
    if let team = teamID, !team.isEmpty {
        return "anchor apple generic and certificate leaf[subject.OU] = \"\(team)\""
    }
    // Ad-hoc / development: no team to pin. The caller decides whether to enforce.
    return "anchor apple generic"
}

/// `reclaimCodeRequirement(teamID:)` additionally pinned to a code-signing
/// `identifier` (the peer's bundle id / executable identifier), so a same-team
/// binary that is not the expected peer is refused too.
public func reclaimCodeRequirement(teamID: String?, identifier: String?) -> String {
    var req = ""
    if let id = identifier, !id.isEmpty {
        req += "identifier \"\(id)\" and "
    }
    return req + reclaimCodeRequirement(teamID: teamID)
}

/// Only ever act on a real BSD disk node: `disk3`, `disk3s5`, `rdisk3s1s2`.
/// ASCII digits only (no Unicode numerals), at most 32 characters, no path
/// separators.
public func isValidBSDName(_ name: String) -> Bool {
    guard name.count <= 32, name.allSatisfy({ $0.isASCII }) else { return false }
    let n = name.hasPrefix("r") ? String(name.dropFirst()) : name
    guard n.hasPrefix("disk") else { return false }
    let rest = n.dropFirst(4)
    guard let first = rest.first, first.isASCII, first.isNumber else { return false }
    return rest.allSatisfy { ($0.isASCII && $0.isNumber) || $0 == "s" }
}

/// The team ID to pin, resolved at runtime: the `RECLAIM_TEAM_ID` env override
/// first (dev loop), then the `ReclaimTeamID` key baked into the bundle's
/// Info.plist by `scripts/build-app.sh` at signing time. A launched `.app` has no
/// environment, so the Info.plist value is what makes the both-ways check work
/// for the shipped bundle. `nil` ⇒ unsigned/ad-hoc: the caller does not pin.
public func reclaimConfiguredTeamID() -> String? {
    if let t = ProcessInfo.processInfo.environment["RECLAIM_TEAM_ID"], !t.isEmpty {
        return t
    }
    // Bundle.main resolves to the enclosing .app for both the app executable and
    // the daemon launched from Contents/MacOS, so both read the app's Info.plist.
    if let t = Bundle.main.object(forInfoDictionaryKey: "ReclaimTeamID") as? String,
        !t.isEmpty
    {
        return t
    }
    return nil
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
