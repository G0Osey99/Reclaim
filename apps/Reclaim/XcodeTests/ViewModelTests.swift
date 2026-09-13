// XCTest view-model tests for a full-Xcode `xcodebuild test` (Phase 6). Not part
// of the SwiftPM package (see XcodeTests/README.md). Mirrors the unit section of
// Sources/ReclaimSelfTest/main.swift.
import XCTest
@testable import ReclaimKit
@testable import ReclaimCore

final class ViewModelTests: XCTestCase {
    func testFilterMapping() {
        var f = FilterState()
        f.family = "image"
        f.exts = ["jpg", "png"]
        f.recoverability = .high
        f.deletedOnly = true
        f.sourceKind = .carved
        f.pathSearch = "DCIM"
        f.sort = .size
        let q = f.resultFilter()
        XCTAssertEqual(q.family, "image")
        XCTAssertEqual(q.exts, ["jpg", "png"])
        XCTAssertEqual(q.minScore, 80)
        XCTAssertTrue(q.deletedOnly)
        XCTAssertEqual(q.engine, "carve")
        XCTAssertEqual(q.pathGlob, "*DCIM*")
        XCTAssertEqual(q.sort, .size)
    }

    func testBlankSearchNoGlob() {
        var g = FilterState()
        g.pathSearch = "   "
        XCTAssertNil(g.resultFilter().pathGlob)
        g.pathSearch = "IMG_*.jpg"
        XCTAssertEqual(g.resultFilter().pathGlob, "IMG_*.jpg")
    }

    func testDestinationValidation() {
        let same = DestinationCheck(
            ok: false, sameDisk: true, freeBytes: 1_000_000, lowSpace: false, reason: "same disk")
        XCTAssertFalse(evaluateDestination(same, allowSameDevice: false).canRecover)
        XCTAssertTrue(evaluateDestination(same, allowSameDevice: true).canRecover)

        let low = DestinationCheck(
            ok: true, sameDisk: false, freeBytes: 100, lowSpace: true, reason: nil)
        let v = evaluateDestination(low, allowSameDevice: false)
        XCTAssertTrue(v.canRecover)
        XCTAssertNotNil(v.warning)
    }
}
