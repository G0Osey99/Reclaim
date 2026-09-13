import SwiftUI
import ReclaimCore
import ReclaimKit

/// S.M.A.R.T. health report (doc 08 §1 Tools, §2.2 health chip; doc 06 §7). Reads
/// device self-monitoring metadata through the same probe the scan plan uses —
/// read-only, no scan. Whole disks expose SMART; partitions inherit their disk's.
struct SmartView: View {
    @EnvironmentObject var model: AppModel
    @State private var bsd: String?
    @State private var probe: SourceProbe?
    @State private var loading = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("S.M.A.R.T.").font(.title2.bold())
            Text("Read the drive's self-monitoring health report. Reads device metadata only — no scan, read-only.")
                .font(.callout).foregroundStyle(.secondary)
            HStack {
                SourcePicker(bsd: $bsd)
                Button("Read") { read() }.disabled(bsd == nil || loading)
            }
            if loading { ProgressView("Reading health…") }
            if let e = errorMessage {
                Label(e, systemImage: "xmark.octagon").foregroundStyle(.red).font(.callout)
            }
            if let p = probe {
                HStack {
                    HealthChip(level: p.health)
                    if p.imageFirst {
                        Label("Image before scanning", systemImage: "externaldrive.badge.plus")
                            .font(.caption.bold())
                            .padding(.horizontal, 8).padding(.vertical, 3)
                            .background(Color.orange.opacity(0.18), in: Capsule())
                            .foregroundStyle(.orange)
                    }
                    Spacer()
                }
                GroupBox {
                    VStack(alignment: .leading, spacing: 6) {
                        KV(key: "SMART", value: p.smart.isEmpty ? "—" : p.smart)
                        KV(key: "Bad sectors seen", value: "\(p.probeBadSectors)")
                        KV(key: "Read speed", value: "\(formatBytes(UInt64(p.readSpeedBps)))/s")
                        KV(key: "Sector size", value: "logical \(p.sectorSizeLogical), physical \(p.sectorSizePhysical)")
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }
                if p.imageFirst {
                    Text("This drive reports as failing. Use Byte-to-byte image first, then scan the image — it minimizes reads on the failing media.")
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else if !loading {
                Text("Select a disk and read its health. Failing drives report here before you scan.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(20)
        .navigationTitle("S.M.A.R.T.")
    }

    private func read() {
        guard let bsd else { return }
        loading = true
        errorMessage = nil
        probe = nil
        Task {
            // Prefer a helper-provided fd (no root needed), as scanning does; the
            // core reads SMART for the fd's disk by its BSD label.
            var dev: DeviceFD?
            if model.helper.status == .enabled {
                dev = try? await model.helper.openDevice(bsd: bsd)
            }
            let spec = dev?.sourceSpec ?? bsd
            let result: Result<SourceProbe, Error>
            do {
                let p = try await Task.detached { try ReclaimCore.probe(source: spec) }.value
                result = .success(p)
            } catch {
                result = .failure(error)
            }
            dev?.close()
            loading = false
            switch result {
            case .success(let p): probe = p
            case .failure(let e): errorMessage = "\(e)"
            }
        }
    }
}
