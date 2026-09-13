// swift-tools-version:5.9
import PackageDescription

// Link the universal reclaim-ffi static lib (built by scripts/gen-ffi-bindings.sh
// / scripts/build-app.sh into Frameworks/) plus the system frameworks the Rust
// core references (IOKit + CoreFoundation for enumeration, libiconv). The path
// is relative to the package root, where SwiftPM invokes the linker.
let ffiLink: [LinkerSetting] = [
    .unsafeFlags([
        "-L", "Frameworks",
        "-lreclaim_ffi",
        "-framework", "IOKit",
        "-framework", "CoreFoundation",
        "-liconv",
    ])
]

let package = Package(
    name: "Reclaim",
    platforms: [.macOS(.v13)],
    products: [
        // ReclaimCore is the Swift package over the UniFFI bindings (doc 08 §5).
        .library(name: "ReclaimCore", targets: ["ReclaimCore"]),
        .executable(name: "Reclaim", targets: ["Reclaim"]),
        .executable(name: "ReclaimHelper", targets: ["ReclaimHelper"]),
    ],
    targets: [
        // The XPC protocol + service name, shared by the app and the helper so
        // neither drifts (doc 03 §7). No dependencies.
        .target(name: "ReclaimHelperProtocol"),
        // C shim exposing the generated FFI header as the `reclaim_ffiFFI` module.
        .target(name: "ReclaimFFI"),
        // The generated Swift bindings.
        .target(name: "ReclaimCore", dependencies: ["ReclaimFFI"]),
        // View models + pure logic (the only unit-tested Swift, doc 08 §D).
        .target(name: "ReclaimKit", dependencies: ["ReclaimCore", "ReclaimHelperProtocol"]),
        // The SwiftUI app.
        .executableTarget(
            name: "Reclaim",
            dependencies: ["ReclaimKit", "ReclaimCore", "ReclaimHelperProtocol"],
            linkerSettings: ffiLink
        ),
        // The privileged helper (XPC daemon) — does not link the core (doc 03 §7).
        .executableTarget(name: "ReclaimHelper", dependencies: ["ReclaimHelperProtocol"]),
        // Assertion-runner tests (XCTest is unavailable under Command Line Tools;
        // see docs/build-log/phase-5.md). Run with `swift run ReclaimSelfTest`.
        .executableTarget(
            name: "ReclaimSelfTest",
            dependencies: ["ReclaimKit", "ReclaimCore"],
            linkerSettings: ffiLink
        ),
    ]
)
