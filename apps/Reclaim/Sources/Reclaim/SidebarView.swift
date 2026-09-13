import SwiftUI
import ReclaimCore
import ReclaimKit

/// The sources / tools / sessions sidebar (doc 08 §1).
struct SidebarView: View {
    @EnvironmentObject var model: AppModel
    @Binding var route: Route

    var body: some View {
        List(selection: Binding(
            get: { route },
            set: { if let r = $0 { route = r } }
        )) {
            Section("Sources") {
                if model.sources.isEmpty {
                    Text("No sources found")
                        .foregroundStyle(.secondary)
                        .font(.callout)
                }
                ForEach(wholeDisks, id: \.bsdName) { disk in
                    SourceRow(disk: disk)
                        .tag(Route.source(disk.bsdName))
                    ForEach(children(of: disk), id: \.bsdName) { part in
                        SourceRow(disk: part, indent: true)
                            .tag(Route.source(part.bsdName))
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

    private var wholeDisks: [SourceSummary] {
        model.sources.filter { $0.whole }
    }
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
            if disk.whole, disk.smart == "FAILING" {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
                    .help("SMART reports failing")
            }
        }
        .padding(.leading, indent ? 14 : 0)
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
