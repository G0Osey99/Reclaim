// XCTest for the scan-options inspector model (GUI Option B) for a full-Xcode
// `xcodebuild test` (Phase 6). Not part of the SwiftPM package (see
// XcodeTests/README.md). Mirrors the ScanPlan section of
// Sources/ReclaimSelfTest/main.swift.
import XCTest
@testable import ReclaimKit
@testable import ReclaimCore

final class ScanPlanTests: XCTestCase {
    func testPresetDetection() {
        XCTAssertEqual(ScanPreset.detect(from: ScanPreset.quick.canonicalOptions()!), .quick)
        XCTAssertEqual(ScanPreset.detect(from: ScanPreset.deep.canonicalOptions()!), .deep)
        XCTAssertEqual(ScanPreset.detect(from: ScanPreset.full.canonicalOptions()!), .full)

        var custom = ScanPreset.full.canonicalOptions()!
        custom.bruteForce = true
        XCTAssertEqual(ScanPreset.detect(from: custom), .custom)
        custom = ScanPreset.full.canonicalOptions()!
        custom.families = ["image"]
        XCTAssertEqual(ScanPreset.detect(from: custom), .custom)
        XCTAssertEqual(ScanPreset.detect(from: makeScanOptions(quick: false, deep: false)), .custom)
    }

    func testCanonicalPasses() {
        let q = ScanPreset.quick.canonicalOptions()!
        XCTAssertTrue(q.quick && !q.deep)
        let d = ScanPreset.deep.canonicalOptions()!
        XCTAssertTrue(!d.quick && d.deep)
        let f = ScanPreset.full.canonicalOptions()!
        XCTAssertTrue(f.quick && f.deep)
    }

    func testFamilyGroups() {
        XCTAssertEqual(ScanFamilies.selectedGroups(from: []).count, ScanFamilyGroup.all.count)
        XCTAssertTrue(ScanFamilies.families(for: Set(ScanFamilyGroup.all.map(\.id))).isEmpty)
        XCTAssertEqual(Set(ScanFamilies.families(for: ["photos"])), Set(["image", "raw"]))
        XCTAssertEqual(ScanFamilies.selectedGroups(from: ["video"]), ["video"])
    }

    func testOptionTablesAndDuration() {
        XCTAssertEqual(ScanBlockSize.label(0), "Auto")
        XCTAssertEqual(ScanCheckpoint.label(5), "Every 5 s")
        XCTAssertEqual(ScanMaxFileSize.label(UInt64(4) << 30), "4 GB")
        XCTAssertEqual(formatDuration(45), "45 s")
        XCTAssertEqual(formatDuration(600), "10 min")
        XCTAssertEqual(formatDuration(0), "—")
    }

    @MainActor func testPlanModelEditing() {
        let plan = ScanPlanModel(preset: .full)
        XCTAssertEqual(plan.preset, .full)
        plan.select(.quick)
        XCTAssertEqual(plan.preset, .quick)
        XCTAssertTrue(plan.options.quick && !plan.options.deep)
        plan.edit { $0.bruteForce = true }
        XCTAssertEqual(plan.preset, .custom)
        plan.edit { $0.bruteForce = false }
        XCTAssertEqual(plan.preset, .quick)
        plan.reset()
        XCTAssertEqual(plan.preset, .full)

        plan.toggleFamilyGroup("other")
        XCTAssertEqual(plan.preset, .custom)
        XCTAssertFalse(plan.isFamilyGroupOn("other"))
        for g in ScanFamilyGroup.all where g.id != "photos" {
            if plan.isFamilyGroupOn(g.id) { plan.toggleFamilyGroup(g.id) }
        }
        plan.toggleFamilyGroup("photos")
        XCTAssertTrue(plan.isFamilyGroupOn("photos"), "cannot deselect the last family group")
    }
}
