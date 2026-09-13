import Foundation
import ReclaimCore
import ReclaimKit

/// Entry point. `Reclaim --smoke <session-dir>` runs a headless integration
/// check against an existing session (built from a golden image) and exits —
/// the CLT-friendly stand-in for an XCUITest launch (see docs/build-log/
/// phase-5.md). Otherwise it launches the SwiftUI app.
@main
enum ReclaimMain {
    static func main() {
        let args = CommandLine.arguments
        if let i = args.firstIndex(of: "--smoke"), i + 1 < args.count {
            exit(runSmoke(sessionDir: args[i + 1]))
        }
        if args.contains("--version") {
            print(version())
            exit(0)
        }
        ReclaimApp.main()
    }

    /// Open a session, page results, count families and render a preview — the
    /// same calls the results screen makes. Exits 0 on success, 1 on failure.
    static func runSmoke(sessionDir: String) -> Int32 {
        do {
            let session = try openSession(dir: sessionDir)
            let total = try session.count()
            let page = try session.query(
                filter: FilterState().resultFilter(), offset: 0, limit: 50)
            let fams = try session.familyCounts(deletedOnly: false)
            print("smoke: session \(sessionDir)")
            print("smoke: \(total) rows, page \(page.items.count)/\(page.total), \(fams.count) families")
            guard total > 0, !page.items.isEmpty else {
                FileHandle.standardError.write(Data("smoke: no results in session\n".utf8))
                return 1
            }
            // Preview the first image that decodes (mirrors the inspector).
            let images = try session.query(
                filter: FilterState(family: "image").resultFilter(), offset: 0, limit: 200)
            var previewed = false
            for r in images.items {
                if let p = try? session.preview(id: r.id, kind: .thumbnail, maxPx: 128),
                    p.bytes.starts(with: [0x89, 0x50, 0x4E, 0x47])
                {
                    print("smoke: previewed \(r.path) → \(p.bytes.count)-byte PNG")
                    previewed = true
                    break
                }
            }
            print("smoke: preview \(previewed ? "OK" : "none (no decodable image)")")
            print("smoke: OK")
            return 0
        } catch {
            FileHandle.standardError.write(Data("smoke: \(error)\n".utf8))
            return 1
        }
    }
}
