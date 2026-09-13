import SwiftUI
import ReclaimCore
import ReclaimKit

/// Source detail + scan plan (doc 08 §2.2). Starts a scan and swaps to the live
/// results screen in the same pane.
struct SourceDetailView: View {
    @EnvironmentObject var model: AppModel
    let bsd: String

    @State private var selectedPartition: Int?
    @State private var depth: ScanDepth = .full
    @State private var showEngines = false
    @StateObject private var scan = ScanController()
    @State private var scanning = false

    enum ScanDepth: String, CaseIterable, Identifiable {
        case quick = "Quick", deep = "Deep", full = "Full"
        var id: String { rawValue }
    }

    var body: some View {
        if scanning, let session = scan.session {
            ScanResultsView(scan: scan, session: session, title: sourceTitle)
        } else {
            plan
        }
    }

    private var source: SourceSummary? { model.sources.first { $0.bsdName == bsd } }
    private var sourceTitle: String { source?.volumeName ?? source?.model ?? bsd }

    private var plan: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                if let p = model.probe, p.source == bsd {
                    if !p.partitions.isEmpty {
                        PartitionBar(
                            partitions: p.partitions, total: p.size, selected: $selectedPartition)
                    }
                    warnings(p)
                    scanConfig(p)
                } else if model.probing {
                    ProgressView("Probing \(bsd)…").padding(.vertical)
                } else {
                    Button("Probe source") { model.probeSelected() }
                }
                Spacer(minLength: 0)
            }
            .padding(20)
        }
        .onAppear {
            model.selectedSourceBSD = bsd
            if model.probe?.source != bsd { model.probeSelected() }
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: source?.internal == true ? "internaldrive" : "externaldrive")
                .font(.system(size: 34)).foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 3) {
                Text(sourceTitle).font(.title2.bold())
                Text(subtitle).foregroundStyle(.secondary).font(.callout)
            }
            Spacer()
            if let p = model.probe, p.source == bsd {
                HealthChip(level: p.health)
            }
        }
    }

    private var subtitle: String {
        guard let s = source else { return bsd }
        var parts = [bsd, formatBytes(s.size)]
        if let bus = s.bus { parts.append(bus) }
        if let fs = s.fsKind { parts.append(fs) }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder private func warnings(_ p: SourceProbe) -> some View {
        if !p.warnings.isEmpty {
            GroupBox {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(p.warnings, id: \.self) { w in
                        Label(w, systemImage: "exclamationmark.triangle")
                            .font(.callout).foregroundStyle(.secondary)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    @ViewBuilder private func scanConfig(_ p: SourceProbe) -> some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 10) {
                KV(key: "Scheme", value: p.scheme)
                KV(
                    key: "Sector size",
                    value: "logical \(p.sectorSizeLogical), physical \(p.sectorSizePhysical)")
                KV(key: "Read speed", value: "\(formatBytes(UInt64(p.readSpeedBps)))/s")
                if let c = p.container { KV(key: "Container", value: c) }
                DisclosureGroup("Engines & options", isExpanded: $showEngines) {
                    Text("APFS · HFS+ · NTFS · exFAT · FAT · ext · ISO metadata, signature carve, and lost-structure — auto-selected. Block size auto; read-only.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }.frame(maxWidth: .infinity, alignment: .leading)
        }

        HStack(spacing: 12) {
            Picker("Depth", selection: $depth) {
                ForEach(ScanDepth.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented).fixedSize()

            if p.imageFirst {
                Button {
                    // Health is red — steer to Image first (doc 08 §2.2).
                } label: {
                    Label("Image first", systemImage: "externaldrive.badge.plus")
                }
                .buttonStyle(.borderedProminent).tint(.orange).disabled(true)
                    .help("Recommended: image this drive before scanning")
                Button(action: startScan) { Label("Scan anyway", systemImage: "magnifyingglass") }
            } else {
                Button(action: startScan) {
                    Label("Scan", systemImage: "magnifyingglass")
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.return, modifiers: .command)
            }
        }
    }

    /// Begin a scan: obtain a source spec (a helper fd for a device when the
    /// helper is enabled; otherwise the BSD name, which needs root) and start.
    private func startScan() {
        let opts: ScanOptions
        switch depth {
        case .quick: opts = Options.scan(quick: true, deep: false)
        case .deep: opts = Options.scan(quick: false, deep: true)
        case .full: opts = Options.scan(quick: true, deep: true)
        }
        Task {
            let spec = await resolveSourceSpec()
            await MainActor.run {
                scan.start(source: spec, sessionDir: nil, opts: opts)
                scanning = true
            }
        }
    }

    /// Prefer a helper-provided fd for a device (doc 03 §7); fall back to the BSD.
    private func resolveSourceSpec() async -> String {
        if model.helper.status == .enabled {
            if let dev = try? await model.helper.openDevice(bsd: bsd) {
                return dev.sourceSpec
            }
        }
        return bsd
    }
}
