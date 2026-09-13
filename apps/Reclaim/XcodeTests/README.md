# Xcode test target (Phase 6)

These XCTest / XCUITest sources are **not** part of the SwiftPM package — XCTest
and Swift Testing are unavailable under Command Line Tools, so including them in
`Package.swift` would break `swift build`. Under Command Line Tools the
equivalent checks run as an assertion executable:

```
scripts/swift-test.sh        # → swift run ReclaimSelfTest
```

When a full Xcode is installed (Phase 6), add these to an XCTest target and run
`xcodebuild test`:

- `ViewModelTests.swift` — the `FilterState → ResultFilter` mapping and the
  destination-validation logic (mirrors the `ReclaimSelfTest` unit section).
- `SmokeUITests.swift` — an `XCUIApplication` launch that points the app at a
  session built from a golden image (`RECLAIM_SMOKE=<dir>`) and asserts the
  results grid populates.

The assertions here are identical to the `ReclaimSelfTest` executable so the two
stay in sync; `ReclaimSelfTest` is the source of truth until the Xcode target
exists.
