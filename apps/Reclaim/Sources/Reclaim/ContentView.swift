import SwiftUI
import ReclaimCore
import ReclaimKit

/// What the detail pane shows.
enum Route: Hashable {
    case welcome
    case source(String)           // BSD name
    case session(String)          // session dir
    case imageTool
    case lostVolumes
    case snapshots
}

/// The sidebar → detail split (doc 08 §1).
struct ContentView: View {
    @EnvironmentObject var model: AppModel
    @State private var route: Route = .welcome

    var body: some View {
        NavigationSplitView {
            SidebarView(route: $route)
                .frame(minWidth: 240)
        } detail: {
            detail
                .frame(minWidth: 720)
        }
        .sheet(isPresented: onboardingBinding) {
            OnboardingView().environmentObject(model)
        }
    }

    /// Show onboarding until the helper is enabled (doc 08 §2.1). Dismissable.
    @State private var onboardingDismissed = false
    private var onboardingBinding: Binding<Bool> {
        Binding(
            get: { !onboardingDismissed && model.helper.status != .enabled },
            set: { if !$0 { onboardingDismissed = true } }
        )
    }

    @ViewBuilder private var detail: some View {
        switch route {
        case .welcome:
            WelcomeView()
        case .source(let bsd):
            SourceDetailView(bsd: bsd)
        case .session(let dir):
            SessionResultsLoader(sessionDir: dir)
        case .imageTool:
            ImageToolView()
        case .lostVolumes:
            LostVolumesView()
        case .snapshots:
            SnapshotBrowserView()
        }
    }
}

/// A plain welcome / empty state.
struct WelcomeView: View {
    @EnvironmentObject var model: AppModel
    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "internaldrive")
                .font(.system(size: 48))
                .foregroundStyle(.secondary)
            Text("Reclaim").font(.largeTitle.bold())
            Text("Read-only disk, file and photo recovery. Core \(model.coreVersion).")
                .foregroundStyle(.secondary)
            Text("Select a source in the sidebar to begin.")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding()
    }
}
