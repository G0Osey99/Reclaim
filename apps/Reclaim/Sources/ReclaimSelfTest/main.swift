// Assertion-runner tests for the app (build guide Phase-5 prompt D). XCTest and
// Swift Testing are unavailable under Command Line Tools (see docs/build-log/
// phase-5.md), so the view-model unit tests and the golden-image UI smoke run
// here as a plain executable: `swift run ReclaimSelfTest`. Exits non-zero on any
// failure. Equivalent XCTest sources for a full-Xcode `xcodebuild test` live in
// apps/Reclaim/XcodeTests/.

import Foundation
import ReclaimCore
import ReclaimHelperProtocol
import ReclaimKit

var failures = 0
func check(_ cond: Bool, _ msg: String) {
    if cond {
        print("  ok: \(msg)")
    } else {
        failures += 1
        print("  FAIL: \(msg)")
    }
}
func section(_ name: String) { print("\n== \(name) ==") }

// MARK: - View-model unit tests (no disk) ----------------------------------

section("FilterState → ResultFilter (filters → SQL)")
do {
    var f = FilterState()
    f.family = "image"
    f.exts = ["jpg", "png"]
    f.recoverability = .high
    f.deletedOnly = true
    f.sourceKind = .carved
    f.pathSearch = "DCIM"
    f.sort = .size
    let q = f.resultFilter()
    check(q.family == "image", "family maps through")
    check(q.exts == ["jpg", "png"], "extensions map through")
    check(q.minScore == 80, "recoverability High → minScore 80")
    check(q.deletedOnly, "deletedOnly maps through")
    check(q.engine == "carve", "sourceKind .carved → engine carve")
    check(q.pathGlob == "*DCIM*", "bare path search → *contains* glob")
    check(q.sort == .size, "sort maps through")

    var g = FilterState()
    g.pathSearch = "  "
    g.recoverability = .any
    let q2 = g.resultFilter()
    check(q2.pathGlob == nil, "blank search → no glob")
    check(q2.minScore == nil, "recoverability Any → no minScore")
    g.pathSearch = "IMG_*.jpg"
    check(g.resultFilter().pathGlob == "IMG_*.jpg", "explicit wildcard preserved")
}

section("Destination validation")
do {
    let same = DestinationCheck(
        ok: false, sameDisk: true, freeBytes: 10_000_000, lowSpace: false,
        reason: "same physical disk")
    let v1 = evaluateDestination(same, allowSameDevice: false)
    check(!v1.canRecover, "same-disk blocks recovery")
    check(v1.blockingReason != nil, "same-disk gives a blocking reason")
    let v1b = evaluateDestination(same, allowSameDevice: true)
    check(v1b.canRecover, "override allows same-disk recovery")

    let low = DestinationCheck(
        ok: true, sameDisk: false, freeBytes: 1000, lowSpace: true, reason: nil)
    let v2 = evaluateDestination(low, allowSameDevice: false)
    check(v2.canRecover, "low space still allows recovery")
    check(v2.warning != nil, "low space produces a warning")

    let good = DestinationCheck(
        ok: true, sameDisk: false, freeBytes: 1_000_000_000, lowSpace: false, reason: nil)
    let v3 = evaluateDestination(good, allowSameDevice: false)
    check(v3.canRecover && v3.warning == nil && v3.blockingReason == nil, "clean destination is OK")
}

section("byte formatting")
check(formatBytes(0) == "0 B", "zero bytes")
check(formatBytes(1536).hasSuffix("KB"), "1536 → KB")

section("ScanPreset ↔ ScanOptions (scan-options inspector)")
do {
    check(ScanPreset.detect(from: ScanPreset.quick.canonicalOptions()!) == .quick, "Quick options → Quick")
    check(ScanPreset.detect(from: ScanPreset.deep.canonicalOptions()!) == .deep, "Deep options → Deep")
    check(ScanPreset.detect(from: ScanPreset.full.canonicalOptions()!) == .full, "Full options → Full")

    let q = ScanPreset.quick.canonicalOptions()!
    check(q.quick && !q.deep, "Quick = metadata only")
    let d = ScanPreset.deep.canonicalOptions()!
    check(!d.quick && d.deep, "Deep = carve only")
    let f = ScanPreset.full.canonicalOptions()!
    check(f.quick && f.deep, "Full = both passes")

    // Any non-pass deviation → Custom.
    var custom = ScanPreset.full.canonicalOptions()!
    custom.bruteForce = true
    check(ScanPreset.detect(from: custom) == .custom, "brute-force deviation → Custom")
    custom = ScanPreset.full.canonicalOptions()!
    custom.families = ["image"]
    check(ScanPreset.detect(from: custom) == .custom, "family restriction → Custom")
    custom = ScanPreset.full.canonicalOptions()!
    custom.rangeStart = 4096
    check(ScanPreset.detect(from: custom) == .custom, "range deviation → Custom")

    // Nothing-to-run is never a named preset.
    check(ScanPreset.detect(from: makeScanOptions(quick: false, deep: false)) == .custom, "no passes → Custom")
}

section("Scan family groups")
do {
    check(ScanFamilies.selectedGroups(from: []).count == ScanFamilyGroup.all.count, "empty families = all groups selected")
    check(ScanFamilies.families(for: Set(ScanFamilyGroup.all.map(\.id))).isEmpty, "all groups → [] (all)")
    let photosOnly = ScanFamilies.families(for: ["photos"])
    check(Set(photosOnly) == Set(["image", "raw"]), "Photos maps to image + raw")
    let groups = ScanFamilies.selectedGroups(from: ["video"])
    check(groups == ["video"], "video family → Video group")
}

section("Scan option tables + duration")
do {
    check(ScanBlockSize.label(0) == "Auto", "block size 0 → Auto")
    check(ScanCheckpoint.label(5) == "Every 5 s", "checkpoint 5 → Every 5 s")
    check(ScanMaxFileSize.label(UInt64(4) << 30) == "4 GB", "max size 4 GiB → 4 GB")
    check(formatDuration(45) == "45 s", "45 s")
    check(formatDuration(600) == "10 min", "600 s → 10 min")
    check(formatDuration(0) == "—", "zero → dash")
}

section("Helper: BSD-name validation")
do {
    for good in ["disk3", "disk3s5", "rdisk3s1s2"] {
        check(isValidBSDName(good), "accepts \(good)")
    }
    let long = "disk" + String(repeating: "1", count: 36)
    for bad in ["disk", "disk\u{0663}", "../disk1", "disk3/", long] {
        check(!isValidBSDName(bad), "rejects \(bad.debugDescription)")
    }
}

section("Helper: code-signing requirement")
do {
    let both = reclaimCodeRequirement(teamID: "ABCDE12345", identifier: "com.reclaim.app")
    check(
        both == "identifier \"com.reclaim.app\" and anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\"",
        "team + identifier requirement")
    check(
        reclaimCodeRequirement(teamID: "ABCDE12345", identifier: nil)
            == reclaimCodeRequirement(teamID: "ABCDE12345"),
        "nil identifier falls back to the team-only requirement")
    check(
        reclaimCodeRequirement(teamID: nil, identifier: "ReclaimHelper")
            == "identifier \"ReclaimHelper\" and anchor apple generic",
        "identifier without a team")
}

section("ScanPlanModel (preset ⇄ edit)")
await MainActor.run {
    let plan = ScanPlanModel(preset: .full)
    check(plan.preset == .full, "starts at Full")
    plan.select(.quick)
    check(plan.preset == .quick && plan.options.quick && !plan.options.deep, "select Quick applies + names it")
    plan.edit { $0.bruteForce = true }
    check(plan.preset == .custom && plan.options.bruteForce, "editing an option → Custom")
    plan.edit { $0.bruteForce = false }
    check(plan.preset == .quick, "reverting the edit snaps back to Quick")
    plan.reset()
    check(plan.preset == .full, "reset → Full")
    // Deselecting a family group narrows carve and flips to Custom.
    plan.toggleFamilyGroup("other")
    check(plan.preset == .custom && !plan.isFamilyGroupOn("other"), "deselect a family → Custom")
    // The last remaining group cannot be removed.
    for g in ScanFamilyGroup.all where g.id != "photos" { if plan.isFamilyGroupOn(g.id) { plan.toggleFamilyGroup(g.id) } }
    check(plan.isFamilyGroupOn("photos"), "one family group always stays selected")
    plan.toggleFamilyGroup("photos")
    check(plan.isFamilyGroupOn("photos"), "cannot deselect the last family group")
}

// MARK: - Golden-image smoke (UI path against a real session) ---------------

func repoGolden() -> String? {
    let candidates = [
        ProcessInfo.processInfo.environment["RECLAIM_GOLDEN"],
        "../../testdata/build/exfat-camera-delete.img",
        "testdata/build/exfat-camera-delete.img",
    ].compactMap { $0 }
    return candidates.first { FileManager.default.fileExists(atPath: $0) }
}

/// A minimal EventSink for the smoke.
final class Counter: EventSink, @unchecked Sendable {
    let lock = NSLock()
    var found = 0
    var done = false
    func onEvent(event: RcEvent) {
        lock.lock(); defer { lock.unlock() }
        switch event {
        case .found: found += 1
        case .done: done = true
        default: break
        }
    }
}

section("Golden-image smoke (scan → query → preview → recover)")
if let golden = repoGolden() {
    do {
        let tmp = NSTemporaryDirectory() + "reclaim-selftest-\(UUID().uuidString)"
        try FileManager.default.createDirectory(atPath: tmp, withIntermediateDirectories: true)
        let img = tmp + "/exfat.img"
        try FileManager.default.copyItem(atPath: golden, toPath: img)
        let sessDir = tmp + "/session"

        let sink = Counter()
        let session = try startScan(
            source: img, sessionDir: sessDir, opts: Options.scan(), sink: sink)
        let summary = try session.wait()
        check(summary?.complete == true, "scan completes")
        check((summary?.totalRows ?? 0) > 0, "scan finds results")
        check(sink.done, "Done event delivered to the sink")

        // Exercise the ResultsModel (a @MainActor view model) on the main actor.
        await MainActor.run {
            let model = ResultsModel(session: session)
            model.reload()
            check(model.total > 0, "results model loads a page")
            check(model.families.contains { $0.family == "image" }, "family counts include images")
            model.filter.family = "image"
            model.reload()
            check(!model.page.isEmpty, "family filter narrows the page")
        }

        // Preview the first decodable image thumbnail (the inspector path),
        // calling the Session directly (nonisolated FFI).
        let images = try session.query(
            filter: FilterState(family: "image").resultFilter(), offset: 0, limit: 200)
        var previewed = false
        var recoverTarget: String?
        for r in images.items {
            if let p = try? session.preview(id: r.id, kind: .thumbnail, maxPx: 128),
                p.bytes.starts(with: [0x89, 0x50, 0x4E, 0x47])
            {
                previewed = true
                recoverTarget = r.id
                break
            }
        }
        check(previewed, "an image previews as a PNG thumbnail")

        // Same-disk refusal, then recover with verify (override for the temp fs).
        if let id = recoverTarget {
            let dest = tmp + "/out"
            do {
                _ = try session.recover(
                    ids: [id], opts: Options.recover(dest: dest, allowSameDevice: false), sink: sink)
                check(false, "same-disk destination should have been refused")
            } catch {
                check("\(error)".contains("same disk") || "\(error)".lowercased().contains("refus"),
                      "same-disk destination refused")
            }
            let outcome = try session.recover(
                ids: [id],
                opts: Options.recover(dest: dest, flat: true, allowSameDevice: true),
                sink: sink)
            check(outcome.recovered == 1, "recovered one file")
            check(outcome.verifyFailed == 0, "verify-after-copy passed")
        }
        try? FileManager.default.removeItem(atPath: tmp)
    } catch {
        check(false, "golden smoke threw: \(error)")
    }
} else {
    print("  (golden image not found — set RECLAIM_GOLDEN or run scripts/gen-fs-images; skipping)")
}

// MARK: - Result -----------------------------------------------------------

print("\n\(failures == 0 ? "ALL TESTS PASSED" : "\(failures) TEST(S) FAILED")")
exit(failures == 0 ? 0 : 1)
