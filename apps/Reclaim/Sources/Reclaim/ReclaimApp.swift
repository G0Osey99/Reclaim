import SwiftUI
import ReclaimKit

/// Cross-view actions the menu bar triggers (kept as notifications so the active
/// screen can respond without a shared focus dance).
enum AppAction {
    static let stop = Notification.Name("reclaim.stop")
    static let recover = Notification.Name("reclaim.recover")
    static let focusFilter = Notification.Name("reclaim.focusFilter")
    static let togglePreview = Notification.Name("reclaim.togglePreview")
}

/// The SwiftUI app (launched from `ReclaimMain.main`). System appearance, SF
/// Symbols, native controls — no custom chrome (doc 08 §4).
struct ReclaimApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
                .frame(minWidth: 1040, minHeight: 640)
                .onAppear { model.refreshSources() }
        }
        .commands {
            // Scan / recover / stop shortcuts (doc 08 §4). These post actions the
            // results screen handles.
            CommandMenu("Scan") {
                Button("Recover…") { post(AppAction.recover) }
                    .keyboardShortcut("r", modifiers: .command)
                Button("Stop") { post(AppAction.stop) }
                    .keyboardShortcut(".", modifiers: .command)
                Divider()
                Button("Find in Results") { post(AppAction.focusFilter) }
                    .keyboardShortcut("f", modifiers: .command)
                Button("Quick Look Preview") { post(AppAction.togglePreview) }
                    .keyboardShortcut(.space, modifiers: [])
            }
            CommandMenu("Tools") {
                // RAID builder is a round-2 item — present but disabled (doc 08 §2.7).
                Button("RAID Builder…") {}.disabled(true)
            }
        }
    }

    private func post(_ name: Notification.Name) {
        NotificationCenter.default.post(name: name, object: nil)
    }
}
