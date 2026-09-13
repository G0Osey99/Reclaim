// XCUITest launch smoke for a full-Xcode `xcodebuild test` (Phase 6). Not part
// of the SwiftPM package (see XcodeTests/README.md). Requires a session built
// from a golden image; pass its path via the RECLAIM_SMOKE launch environment.
import XCTest

final class SmokeUITests: XCTestCase {
    func testLaunchAgainstGoldenSession() throws {
        let sessionDir = ProcessInfo.processInfo.environment["RECLAIM_SMOKE"]
        try XCTSkipIf(sessionDir == nil, "set RECLAIM_SMOKE to a golden-image session dir")

        let app = XCUIApplication()
        app.launchEnvironment["RECLAIM_OPEN_SESSION"] = sessionDir
        app.launch()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 10))
        // The results grid / list should populate from the session. The exact
        // accessibility identifiers are assigned in the SwiftUI views; assert the
        // window exists as a minimal liveness check.
        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 5))
    }
}
