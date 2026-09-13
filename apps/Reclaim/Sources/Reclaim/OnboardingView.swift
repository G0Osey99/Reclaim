import SwiftUI
import AppKit
import ReclaimKit

/// First-launch onboarding (doc 08 §2.1): explain the read-only guarantee,
/// install the helper (`SMAppService`), check Full Disk Access, and show the
/// "stop using the drive" tip.
///
/// SMAppService has no status-change callback (Apple DTS, FB17671405), so the
/// two gates are re-checked live — on a short timer while this sheet is open and
/// whenever the app regains focus (e.g. returning from System Settings) — so the
/// pills flip to done on their own once the user approves, instead of showing a
/// stale "operation not permitted".
struct OnboardingView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    private let poll = Timer.publish(every: 1.5, on: .main, in: .common).autoconnect()

    private var helper: HelperClient { model.helper }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack {
                Image(systemName: "lock.shield").font(.title)
                Text("Welcome to Reclaim").font(.title.bold())
            }
            Text("Reclaim never writes to the drive you are recovering — every device is opened read-only, by construction.")
                .foregroundStyle(.secondary)

            GroupBox {
                helperStep
                Divider()
                fdaStep
            }

            GroupBox {
                Label {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Stop using the drive you lost data on").font(.headline)
                        Text("Continued use (especially on an internal SSD, where TRIM erases freed blocks) overwrites recoverable data. Recover to a *different* drive.")
                            .font(.callout).foregroundStyle(.secondary)
                    }
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.yellow)
                }
            }

            // First-run authorization / license notice (docs/plan/11 §5, docs/EULA.md).
            Text("Recover only data you own or are authorized to access — doing otherwise may be illegal. Reclaim reads only volumes already unlocked by macOS or that you unlock yourself; it never breaks encryption. Provided under the Apache-2.0 license, with no warranty. By continuing you agree to the [license and use notice](https://github.com/G0Osey99/reclaim/blob/main/docs/EULA.md).")
                .font(.footnote).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            HStack {
                if helper.isReady {
                    Label("All set", systemImage: "checkmark.seal.fill").foregroundStyle(.green)
                }
                Spacer()
                Button(helper.isReady ? "Continue" : "Continue anyway") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 580)
        .task { await refresh() }
        .onReceive(poll) { _ in if !helper.isReady { Task { await refresh() } } }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await refresh() }
        }
    }

    // MARK: helper step

    @ViewBuilder private var helperStep: some View {
        stepRow(done: helper.status == .enabled, title: "Install the privileged helper") {
            switch helper.status {
            case .enabled:
                Text("Installed").foregroundStyle(.green)
            case .requiresApproval:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Button("Open Login Items…") { helper.openLoginItems() }
                }
            case .notInstalled:
                Button("Install…") { helper.install() }
            case .failed:
                Button("Retry") { helper.install() }
            }
        } detail: {
            switch helper.status {
            case .requiresApproval:
                Text("Turn **Reclaim** on under “Allow in the Background” in System Settings → General → Login Items & Extensions. This screen updates automatically.")
            case .failed(let msg):
                Text(msg).foregroundStyle(.red)
            default:
                Text("A small root helper opens devices read-only and hands back a file descriptor. It never parses your data.")
            }
        }
    }

    // MARK: FDA step

    @ViewBuilder private var fdaStep: some View {
        let helperReady = helper.status == .enabled
        stepRow(done: helper.fullDiskAccess, title: "Grant Full Disk Access") {
            if helper.fullDiskAccess {
                Text("Granted").foregroundStyle(.green)
            } else if !helperReady {
                Text("After the helper").foregroundStyle(.secondary)
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Button("Open Privacy Settings…") { helper.openFullDiskAccessSettings() }
                }
            }
        } detail: {
            if helper.fullDiskAccess {
                Text("Reclaim can read the boot device.")
            } else if !helperReady {
                Text("Reading the raw boot device also needs Full Disk Access — available once the helper is enabled.")
            } else {
                Text("Add **Reclaim** in System Settings → Privacy & Security → Full Disk Access, then return here. This screen updates automatically.")
            }
        }
    }

    @ViewBuilder
    private func stepRow<Trailing: View, Detail: View>(
        done: Bool,
        title: String,
        @ViewBuilder trailing: () -> Trailing,
        @ViewBuilder detail: () -> Detail
    ) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: done ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(done ? .green : .secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.headline)
                detail().font(.callout).foregroundStyle(.secondary)
            }
            Spacer()
            trailing()
        }
        .padding(.vertical, 4)
    }

    private func refresh() async {
        await helper.refresh(bootWholeDisk: model.bootWholeDisk)
    }
}
