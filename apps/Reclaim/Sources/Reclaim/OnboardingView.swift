import SwiftUI
import ReclaimKit

/// First-launch onboarding (doc 08 §2.1): explain the read-only guarantee,
/// install the helper (`SMAppService`), check Full Disk Access, and show the
/// "stop using the drive" tip.
struct OnboardingView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var busy = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack {
                Image(systemName: "lock.shield").font(.title)
                Text("Welcome to Reclaim").font(.title.bold())
            }
            Text("Reclaim never writes to the drive you are recovering — every device is opened read-only, by construction.")
                .foregroundStyle(.secondary)

            GroupBox {
                stepRow(
                    done: model.helper.status == .enabled,
                    title: "Install the privileged helper",
                    detail: helperDetail
                ) {
                    if model.helper.status == .enabled {
                        Text("Installed").foregroundStyle(.green)
                    } else {
                        Button(model.helper.status == .requiresApproval ? "Approve in Settings" : "Install…") {
                            model.helper.install()
                        }
                    }
                }
                Divider()
                stepRow(
                    done: model.helper.fullDiskAccess,
                    title: "Grant Full Disk Access",
                    detail: "Reading the raw boot device requires Full Disk Access for Reclaim."
                ) {
                    if model.helper.fullDiskAccess {
                        Text("Granted").foregroundStyle(.green)
                    } else {
                        Button("Open Privacy Settings…") {
                            model.helper.openFullDiskAccessSettings()
                        }
                    }
                }
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

            HStack {
                if case let .failed(msg) = model.helper.status {
                    Label(msg, systemImage: "xmark.octagon").foregroundStyle(.red).font(.callout)
                }
                Spacer()
                Button("Continue") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 560)
        .onAppear { recheck() }
        .task { await probeFDA() }
    }

    private var helperDetail: String {
        switch model.helper.status {
        case .requiresApproval: return "Approve Reclaim in System Settings → Login Items."
        default: return "A small root helper opens devices read-only and hands back a file descriptor."
        }
    }

    @ViewBuilder
    private func stepRow<Trailing: View>(
        done: Bool, title: String, detail: String, @ViewBuilder trailing: () -> Trailing
    ) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: done ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(done ? .green : .secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.headline)
                Text(detail).font(.callout).foregroundStyle(.secondary)
            }
            Spacer()
            trailing()
        }
        .padding(.vertical, 4)
    }

    private func recheck() { model.helper.refreshStatus() }

    private func probeFDA() async {
        if let boot = model.bootWholeDisk, model.helper.status == .enabled {
            await model.helper.probeFullDiskAccess(bootWholeDisk: boot)
        }
    }
}
