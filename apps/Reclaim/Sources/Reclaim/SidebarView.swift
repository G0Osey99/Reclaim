import SwiftUI
import ReclaimCore
import ReclaimKit

/// The sources / tools / sessions sidebar (doc 08 §1), "Scan options inspector"
/// layout (GUI Option B): sources split into Internal / External with a SMART
/// health dot, and a fuller Tools section (image, lost volumes, snapshots,
/// S.M.A.R.T., and a round-2 RAID builder shown disabled).
struct SidebarView: View {
    @EnvironmentObject var model: AppModel
    @Binding var route: Route

    var body: some View {
        List(selection: Binding(
            get: { route },
            set: { if let r = $0 { route = r } }
        )) {
            if model.sources.isEmpty {
                Section("Sources") {
                    Text("No sources found")
                        .foregroundStyle(.secondary)
                        .font(.callout)
                }
            } else {
                if !internalDisks.isEmpty {
                    Section("Internal") { diskRows(internalDisks) }
                }
                if !externalDisks.isEmpty {
                    Section("External") { diskRows(externalDisks) }
                }
                // Fallback if nothing enumerated as a whole disk (e.g. bare volumes).
                if internalDisks.isEmpty, externalDisks.isEmpty {
                    Section("Sources") {
                        ForEach(model.sources, id: \.bsdName) { s in
                            SourceRow(disk: s).tag(Route.source(s.bsdName))
                        }
                    }
                }
            }

            Section("Tools") {
                Label("Byte-to-byte image", systemImage: "externaldrive.badge.timemachine")
                    .tag(Route.imageTool)
                Label("Lost volumes", systemImage: "questionmark.folder")
                    .tag(Route.lostVolumes)
                Label("Snapshot browser", systemImage: "clock.arrow.circlepath")
                    .tag(Route.snapshots)
                Label("S.M.A.R.T.", systemImage: "heart.text.square")
                    .tag(Route.smart)
                disabledTool("RAID builder", systemImage: "square.stack.3d.up", badge: "Soon")
            }

            if !model.sessions.isEmpty {
                Section("Sessions") {
                    ForEach(model.sessions) { s in
                        Label(s.name, systemImage: "tray.full")
                            .tag(Route.session(s.dir))
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .toolbar {
            ToolbarItem {
                Button {
                    model.refreshSources()
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .help("Rescan sources")
            }
        }
    }

    @ViewBuilder private func diskRows(_ disks: [SourceSummary]) -> some View {
        ForEach(disks, id: \.bsdName) { disk in
            SourceRow(disk: disk)
                .tag(Route.source(disk.bsdName))
            ForEach(children(of: disk), id: \.bsdName) { part in
                SourceRow(disk: part, indent: true)
                    .tag(Route.source(part.bsdName))
            }
        }
    }

    /// A disabled, non-selectable tool row (round-2 features, doc 08 §2.7).
    private func disabledTool(_ title: String, systemImage: String, badge: String) -> some View {
        HStack {
            Label(title, systemImage: systemImage)
            Spacer()
            Text(badge)
                .font(.caption2.weight(.semibold))
                .padding(.horizontal, 5).padding(.vertical, 1)
                .background(Color.secondary.opacity(0.15), in: Capsule())
        }
        .foregroundStyle(.tertiary)
        .help("Coming in a future update")
    }

    private var wholeDisks: [SourceSummary] { model.sources.filter { $0.whole } }
    private var internalDisks: [SourceSummary] { wholeDisks.filter { $0.internal == true } }
    private var externalDisks: [SourceSummary] { wholeDisks.filter { $0.internal != true } }
    private func children(of disk: SourceSummary) -> [SourceSummary] {
        model.sources.filter { !$0.whole && $0.wholeDisk == disk.bsdName }
    }
}

/// One row in the sources list.
struct SourceRow: View {
    let disk: SourceSummary
    var indent: Bool = false

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).lineLimit(1)
                Text(subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            if disk.whole, let dot = healthDot {
                Circle()
                    .fill(dot.color)
                    .frame(width: 7, height: 7)
                    .help(dot.help)
            }
        }
        .padding(.leading, indent ? 14 : 0)
    }

    /// A SMART health dot for whole disks (doc 08 §1). `SourceSummary.smart` is
    /// `ok` / `FAILING` / `n/a` / `—`.
    private var healthDot: (color: Color, help: String)? {
        switch disk.smart.uppercased() {
        case "OK": return (.green, "SMART: healthy")
        case "FAILING": return (.red, "SMART reports failing")
        default: return nil
        }
    }

    private var icon: String {
        if disk.isApfsContainer { return "cylinder.split.1x2" }
        if !disk.whole { return "externaldrive.fill" }
        return disk.internal == true ? "internaldrive" : "externaldrive"
    }
    private var title: String {
        if !disk.whole { return disk.volumeName ?? disk.bsdName }
        return disk.model ?? disk.bsdName
    }
    private var subtitle: String {
        var parts: [String] = [formatBytes(disk.size)]
        if let bus = disk.bus { parts.append(bus) }
        if let fs = disk.fsKind { parts.append(fs) }
        return parts.joined(separator: " · ")
    }
}
