import SwiftUI
import ReclaimCore
import ReclaimKit

/// Source detail + scan plan (doc 08 §2.2), "Scan options inspector" layout
/// (GUI Option B): a Quick/Deep/Full **preset** control on the plan and a docked,
/// collapsible **Scan Options** inspector on the right. Starts a scan and swaps to
/// the live results screen in the same pane.
struct SourceDetailView: View {
    @EnvironmentObject var model: AppModel
    let bsd: String

    @State private var selectedPartition: Int?
    @State private var showInspector = true
    @StateObject private var plan = ScanPlanModel()
    @StateObject private var scan = ScanController()
    @State private var scanning = false

    var body: some View {
        if scanning, let session = scan.session {
            ScanResultsView(scan: scan, session: session, title: sourceTitle)
        } else {
            planView
        }
    }

    private var source: SourceSummary? { model.sources.first { $0.bsdName == bsd } }
    private var sourceTitle: String { source?.volumeName ?? source?.model ?? bsd }
    private var probe: SourceProbe? {
        guard let p = model.probe, p.source == bsd else { return nil }
        return p
    }

    private var planView: some View {
        HStack(spacing: 0) {
            planColumn
                .frame(minWidth: 420, maxWidth: .infinity)
            if showInspector {
                Divider()
                ScanOptionsInspector(plan: plan, probe: probe)
                    .frame(width: 300)
            }
        }
        .toolbar {
            ToolbarItem {
                Button {
                    showInspector.toggle()
                } label: {
                    Image(systemName: "sidebar.right")
                }
                .help(showInspector ? "Hide scan options" : "Show scan options")
            }
        }
        .task(id: bsd) {
            // Auto-probe on selection AND whenever the selected source changes.
            // (`.task(id:)` re-runs on id change; `.onAppear` only fired once, so
            // switching sources left the stale probe and showed a manual button.)
            selectedPartition = nil
            model.selectedSourceBSD = bsd
            if model.probe?.source != bsd { model.probeSelected() }
        }
    }

    /// Shown only when an automatic probe could not complete (e.g. the helper
    /// isn't set up yet, or the device needs privileges the app lacks).
    @ViewBuilder private var probeFallback: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let e = model.errorMessage {
                Label(e, systemImage: "exclamationmark.triangle")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if model.helper.status != .enabled {
                Label(
                    "Reading a device needs the privileged helper. Finish setup in onboarding, then retry.",
                    systemImage: "lock.shield")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Button("Retry probe") { model.probeSelected() }
        }
        .padding(.vertical)
    }

    private var planColumn: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let e = scan.errorMessage {
                    Label(e, systemImage: "xmark.octagon")
                        .foregroundStyle(.red).font(.callout)
                        .fixedSize(horizontal: false, vertical: true)
                }
                header
                if let p = probe {
                    if !p.partitions.isEmpty {
                        PartitionBar(
                            partitions: p.partitions, total: p.size, selected: $selectedPartition)
                    }
                    warnings(p)
                    infoBox(p)
                    presetControl(p)
                    actions(p)
                } else if model.probing {
                    ProgressView("Probing \(bsd)…").padding(.vertical)
                } else {
                    probeFallback
                }
                Spacer(minLength: 0)
            }
            .padding(20)
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
            if let p = probe {
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

    private func infoBox(_ p: SourceProbe) -> some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 10) {
                KV(key: "Scheme", value: p.scheme)
                KV(
                    key: "Sector size",
                    value: "logical \(p.sectorSizeLogical), physical \(p.sectorSizePhysical)")
                KV(key: "Read speed", value: "\(formatBytes(UInt64(p.readSpeedBps)))/s")
                if let c = p.container { KV(key: "Container", value: c) }
            }.frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// The preset segmented control + a one-line summary/estimate. "Custom" is
    /// derived (shown as a chip) rather than directly selectable.
    private func presetControl(_ p: SourceProbe) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Text("Preset").font(.callout).foregroundStyle(.secondary)
                Picker("Preset", selection: Binding(
                    get: { plan.preset },
                    set: { plan.select($0) }
                )) {
                    ForEach(ScanPreset.selectable) { Text($0.label).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                if plan.preset == .custom {
                    Text("Custom")
                        .font(.caption.bold())
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(Color.accentColor.opacity(0.18), in: Capsule())
                        .foregroundStyle(Color.accentColor)
                }
                if !showInspector {
                    Button("Options…") { showInspector = true }
                        .controlSize(.small)
                }
            }
            HStack(spacing: 6) {
                Text(plan.preset.summary)
                if let est = estimatedScanText(
                    preset: plan.preset, sizeBytes: p.size, readBps: p.readSpeedBps) {
                    Text("·").foregroundStyle(.tertiary)
                    Text(est)
                }
            }
            .font(.caption).foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder private func actions(_ p: SourceProbe) -> some View {
        HStack(spacing: 12) {
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
                Spacer()
                Button(action: startScan) {
                    Label("Scan", systemImage: "magnifyingglass")
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.return, modifiers: .command)
            }
        }
    }

    /// Begin a scan with the plan's options: obtain a source spec (a helper fd for
    /// a device when the helper is enabled; otherwise the BSD name, which needs
    /// root) and start.
    private func startScan() {
        let opts = plan.options
        Task {
            let dev = await resolveDevice()
            await MainActor.run {
                // The controller keeps `dev` alive for the session (the core
                // re-dups the fd for preview/recover). A failed start stays on
                // the plan and shows `scan.errorMessage`.
                scan.start(source: dev?.sourceSpec ?? bsd, device: dev, sessionDir: nil, opts: opts)
                scanning = scan.session != nil
            }
        }
    }

    /// Prefer a helper-provided fd for a device (doc 03 §7); nil ⇒ use the BSD.
    private func resolveDevice() async -> DeviceFD? {
        guard model.helper.status == .enabled else { return nil }
        return try? await model.helper.openDevice(bsd: bsd)
    }
}
