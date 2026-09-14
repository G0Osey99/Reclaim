import SwiftUI
import AppKit
import ReclaimCore
import ReclaimKit

/// A reusable source picker over the enumerated whole disks + partitions.
struct SourcePicker: View {
    @EnvironmentObject var model: AppModel
    @Binding var bsd: String?
    var body: some View {
        Picker("Source", selection: $bsd) {
            Text("Select…").tag(String?.none)
            ForEach(model.sources, id: \.bsdName) { s in
                Text("\(s.bsdName) — \(s.volumeName ?? s.model ?? "—") (\(formatBytes(s.size)))")
                    .tag(String?.some(s.bsdName))
            }
        }
    }
}

// MARK: - Byte-to-byte image tool (doc 08 §2.5)

struct ImageToolView: View {
    @EnvironmentObject var model: AppModel
    @State private var bsd: String?
    @State private var dest = ""
    @State private var zstd = false
    @State private var job: ImageJob?
    @State private var progress: (done: UInt64, total: UInt64, bad: UInt64) = (0, 0, 0)
    @State private var running = false
    @State private var doneInfo: ImageDone?
    @State private var errorMessage: String?

    var body: some View {
        Form {
            Section("Byte-to-byte image") {
                Text("Copy a source sector-by-sector to an image file, retrying and mapping bad sectors. Recommended before scanning a failing drive.")
                    .font(.callout).foregroundStyle(.secondary)
                SourcePicker(bsd: $bsd)
                HStack {
                    TextField("Destination .img", text: $dest).textFieldStyle(.roundedBorder)
                    Button("Choose…") { chooseDest() }
                }
                Toggle("Compress (zstd)", isOn: $zstd)
            }
            Section("Progress") {
                badSectorStrip
                if running {
                    ProgressView(value: Double(progress.done), total: Double(max(progress.total, 1)))
                    Text("\(formatBytes(progress.done)) / \(formatBytes(progress.total)) · \(progress.bad) bad sector(s)")
                        .font(.caption).foregroundStyle(.secondary)
                } else if let d = doneInfo {
                    Label(d.complete ? "Imaging complete" : "Interrupted",
                          systemImage: d.complete ? "checkmark.circle.fill" : "pause.circle")
                        .foregroundStyle(d.complete ? .green : .orange)
                    if let h = d.wholeHash { KV(key: "hash", value: h) }
                    Button("Scan this image") { scanImage(d.imagePath) }
                }
                if let e = errorMessage {
                    Label(e, systemImage: "xmark.octagon").foregroundStyle(.red).font(.callout)
                }
            }
            Section {
                HStack {
                    Spacer()
                    if running {
                        Button("Cancel") { job?.cancel() }
                    } else {
                        Button("Start imaging") { start() }
                            .buttonStyle(.borderedProminent)
                            .disabled(bsd == nil || dest.isEmpty)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Byte-to-byte image")
    }

    /// A 1-pixel-per-N-blocks strip (green done / grey remaining / red bad),
    /// approximated from the cumulative counters the imager reports (doc 08 §2.5).
    private var badSectorStrip: some View {
        GeometryReader { geo in
            let frac = progress.total > 0 ? Double(progress.done) / Double(progress.total) : 0
            let badFrac = progress.total > 0 ? min(1, Double(progress.bad) * 512 / Double(progress.total)) : 0
            ZStack(alignment: .leading) {
                Rectangle().fill(Color.secondary.opacity(0.2))
                Rectangle().fill(Color.green.opacity(0.6)).frame(width: geo.size.width * frac)
                if badFrac > 0 {
                    Rectangle().fill(Color.red).frame(width: max(2, geo.size.width * badFrac))
                }
            }.clipShape(RoundedRectangle(cornerRadius: 3))
        }
        .frame(height: 14)
    }

    private func chooseDest() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(bsd ?? "disk").img"
        panel.canCreateDirectories = true
        if panel.runModal() == .OK, let url = panel.url { dest = url.path }
    }

    private func start() {
        guard let bsd else { return }
        running = true; errorMessage = nil; doneInfo = nil
        let sink = EventForwarder { ev in
            if case let .imageProgress(_, done, total, bad) = ev { progress = (done, total, bad) }
        }
        let dst = dest
        Task {
            // Keep the helper fd open until the job has finished with it.
            let dev = await deviceFor(bsd)
            defer { dev?.close() }
            let spec = dev?.sourceSpec ?? bsd
            do {
                let j = try startImage(source: spec, dest: dst, opts: Options.image(zstd: zstd), sink: sink)
                job = j
                let info: ImageDone = try await Task.detached { try j.wait() }.value
                running = false; doneInfo = info
            } catch {
                running = false; errorMessage = "\(error)"
            }
        }
    }

    private func scanImage(_ path: String) {
        // Reuse the results screen by opening a fresh scan on the produced image.
        errorMessage = "Open the produced image with File ▸ Scan Image (or drag it in)."
        _ = path
    }

    private func deviceFor(_ bsd: String) async -> DeviceFD? {
        guard model.helper.status == .enabled else { return nil }
        return try? await model.helper.openDevice(bsd: bsd)
    }
}

// MARK: - Lost volumes (doc 08 §2.6)

struct LostVolumesView: View {
    @EnvironmentObject var model: AppModel
    @State private var bsd: String?
    @State private var proposals: [ProposalInfo] = []
    @State private var scanning = false
    @State private var errorMessage: String?
    @State private var adopted: Session?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Lost volumes").font(.title2.bold())
            Text("Scan for filesystem structures whose partition entry is gone (a wiped GPT, a reformatted volume). Adopt a proposal to scan it as a source.")
                .font(.callout).foregroundStyle(.secondary)
            HStack {
                SourcePicker(bsd: $bsd)
                Button("Detect") { detect() }.disabled(bsd == nil || scanning)
            }
            if scanning { ProgressView("Scanning structures…") }
            if let e = errorMessage { Label(e, systemImage: "xmark.octagon").foregroundStyle(.red) }
            List(Array(proposals.enumerated()), id: \.offset) { _, p in
                VStack(alignment: .leading, spacing: 3) {
                    HStack {
                        Text(p.fs.uppercased()).font(.headline)
                        Spacer()
                        Text("conf \(String(format: "%.2f", p.confidence))").foregroundStyle(.secondary)
                        Button("Adopt & scan") { adopt(p) }
                    }
                    Text(p.evidence).font(.caption).foregroundStyle(.secondary)
                    Text(String(format: "start 0x%llx · %@", p.start, formatBytes(p.len)))
                        .font(.caption.monospaced()).foregroundStyle(.secondary)
                }.padding(.vertical, 2)
            }
            if let adopted {
                NavigationLink("Open adopted volume results") {
                    BrowseResultsView(session: adopted, title: "Adopted volume")
                }
            }
        }
        .padding(20)
        .navigationTitle("Lost volumes")
    }

    private func detect() {
        guard let bsd else { return }
        scanning = true; errorMessage = nil
        Task {
            let dev = await deviceFor(bsd)
            defer { dev?.close() }
            let spec = dev?.sourceSpec ?? bsd
            do {
                let list = try await Task.detached { try volumes(source: spec, sessionDir: nil) }.value
                proposals = list; scanning = false
            } catch { errorMessage = "\(error)"; scanning = false }
        }
    }

    private func adopt(_ p: ProposalInfo) {
        // Adopting = scanning the `session:.../volume/N` spec via a fresh scan.
        let sc = ScanController()
        sc.start(source: p.sourceSpec, sessionDir: nil, opts: Options.scan())
        adopted = sc.session
    }

    private func deviceFor(_ bsd: String) async -> DeviceFD? {
        guard model.helper.status == .enabled else { return nil }
        return try? await model.helper.openDevice(bsd: bsd)
    }
}

// MARK: - Snapshot browser (doc 08 §2.8) — no root needed

struct SnapshotBrowserView: View {
    @EnvironmentObject var model: AppModel
    @State private var bsd: String?
    @State private var list: SnapshotList?
    @State private var diff: [DiffEntry] = []
    @State private var errorMessage: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Snapshot browser").font(.title2.bold())
            Text("List APFS snapshots and reachable checkpoints, and diff a point against the current volume. Reads metadata only — no root required.")
                .font(.callout).foregroundStyle(.secondary)
            HStack {
                SourcePicker(bsd: $bsd)
                Button("List") { load() }.disabled(bsd == nil)
            }
            if let e = errorMessage { Label(e, systemImage: "xmark.octagon").foregroundStyle(.red) }
            if let list {
                Text("Volumes: \(list.volumes.joined(separator: ", "))").font(.callout)
                Text("Recoverable checkpoints: \(list.recoverableXids.count)").font(.callout).foregroundStyle(.secondary)
                List(Array(list.points.enumerated()), id: \.offset) { _, p in
                    HStack {
                        Image(systemName: p.kind == .snapshot ? "camera" : "clock")
                        Text(p.name.isEmpty ? "xid \(p.xid)" : p.name)
                        Spacer()
                        Button("Diff vs now") { doDiff(p.xid) }
                    }
                }
                if !diff.isEmpty {
                    Text("Files present then, absent now (\(diff.count)):").font(.headline)
                    List(Array(diff.enumerated()), id: \.offset) { _, e in
                        HStack { Text(e.path); Spacer(); Text(formatBytes(e.size)).foregroundStyle(.secondary) }
                    }
                }
            }
        }
        .padding(20)
        .navigationTitle("Snapshot browser")
    }

    private func load() {
        guard let bsd else { return }
        errorMessage = nil; diff = []
        Task {
            let dev = await deviceFor(bsd)
            defer { dev?.close() }
            let spec = dev?.sourceSpec ?? bsd
            do { list = try await Task.detached { try snapshots(source: spec) }.value }
            catch { errorMessage = "\(error)" }
        }
    }
    private func doDiff(_ xid: UInt64) {
        guard let bsd else { return }
        Task {
            let dev = await deviceFor(bsd)
            defer { dev?.close() }
            let spec = dev?.sourceSpec ?? bsd
            do { diff = try await Task.detached { try snapshotDiff(source: spec, fromXid: xid) }.value }
            catch { errorMessage = "\(error)" }
        }
    }
    private func deviceFor(_ bsd: String) async -> DeviceFD? {
        guard model.helper.status == .enabled else { return nil }
        return try? await model.helper.openDevice(bsd: bsd)
    }
}
