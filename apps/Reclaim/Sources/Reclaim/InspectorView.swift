import SwiftUI
import ReclaimCore
import ReclaimKit

/// The right-hand inspector: preview + metadata + extents/validity (doc 08 §2.3).
struct InspectorView: View {
    @ObservedObject var results: ResultsModel
    @State private var previewKind: PreviewKind = .thumbnail
    @State private var image: NSImage?
    @State private var text: String?
    @State private var loadingFor: String?

    var body: some View {
        Group {
            if let rec = results.focusedRecord {
                ScrollView { content(rec) }
            } else {
                VStack(spacing: 6) {
                    Image(systemName: "sidebar.right").font(.title).foregroundStyle(.secondary)
                    Text("Select a result").foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: AppAction.togglePreview)) { _ in
            if let rec = results.focusedRecord { openExternally(rec) }
        }
    }

    @ViewBuilder private func content(_ rec: ResultRecord) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(rec.path.split(separator: "/").last.map(String.init) ?? rec.path)
                .font(.headline).lineLimit(2)

            previewControl(rec)
            previewArea(rec)

            GroupBox("Metadata") {
                VStack(alignment: .leading, spacing: 4) {
                    metaRow("Format", rec.format)
                    metaRow("Family", rec.family)
                    if let n = rec.name { metaRow("Name", n) }
                    if let d = rec.date { metaRow("Date", d) }
                    if let m = rec.model { metaRow("Model", m) }
                    metaRow("Engine", rec.sourceKind)
                    if let s = rec.state { metaRow("State", s) }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }

            GroupBox("Validity & extents") {
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("Validity").foregroundStyle(.secondary).frame(width: 90, alignment: .trailing)
                        validityBadge(rec.validity)
                        Spacer()
                    }
                    metaRow("Recoverability", "\(rec.score)/100")
                    metaRow("Size", formatBytes(rec.len))
                    metaRow("Extents", "\(rec.extents.count) run(s)")
                    ForEach(Array(rec.extents.prefix(6).enumerated()), id: \.offset) { _, e in
                        Text(String(format: "0x%llx · %@", e.offset, formatBytes(e.len)))
                            .font(.caption.monospaced()).foregroundStyle(.secondary)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }

            Button {
                openExternally(rec)
            } label: {
                Label("Open with default app", systemImage: "arrow.up.forward.app")
            }
            .help("Extracts the file to the session's temp dir and opens it")
        }
        .padding(12)
        .task(id: rec.id) { await loadPreview(rec) }
        .onChange(of: previewKind) { _ in Task { await loadPreview(rec) } }
    }

    @ViewBuilder private func previewControl(_ rec: ResultRecord) -> some View {
        Picker("", selection: $previewKind) {
            Text("Image").tag(PreviewKind.thumbnail)
            Text("Text").tag(PreviewKind.text)
            Text("Hex").tag(PreviewKind.hex)
        }.pickerStyle(.segmented)
    }

    @ViewBuilder private func previewArea(_ rec: ResultRecord) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8).fill(Color.secondary.opacity(0.1))
            if loadingFor == rec.id {
                ProgressView()
            } else if previewKind == .thumbnail, let image {
                Image(nsImage: image).resizable().scaledToFit().padding(6)
            } else if previewKind != .thumbnail, let text {
                ScrollView {
                    Text(text).font(.system(.caption, design: .monospaced))
                        .frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled)
                }.padding(6)
            } else {
                VStack(spacing: 4) {
                    Image(systemName: "eye.slash").foregroundStyle(.secondary)
                    Text("No inline preview").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .frame(height: 200)
    }

    private func metaRow(_ k: String, _ v: String) -> some View {
        HStack(alignment: .top) {
            Text(k).foregroundStyle(.secondary).frame(width: 90, alignment: .trailing)
            Text(v).textSelection(.enabled)
            Spacer()
        }.font(.callout)
    }

    private func validityBadge(_ v: String) -> some View {
        Text(v)
            .font(.caption.bold()).padding(.horizontal, 8).padding(.vertical, 2)
            .background((v == "full" ? Color.green : .orange).opacity(0.2), in: Capsule())
            .foregroundStyle(v == "full" ? .green : .orange)
    }

    private func loadPreview(_ rec: ResultRecord) async {
        image = nil; text = nil
        loadingFor = rec.id
        let id = rec.id
        let session = results.session
        let kind = previewKind
        let out: (Data?, String?) = await Task.detached {
            guard let p = try? session.preview(id: id, kind: kind, maxPx: 512) else { return (nil, nil) }
            if kind == .thumbnail { return (Data(p.bytes), nil) }
            return (nil, String(data: Data(p.bytes), encoding: .utf8))
        }.value
        if loadingFor == id {
            image = out.0.flatMap { NSImage(data: $0) }
            text = out.1
            loadingFor = nil
        }
    }

    /// Extract to the session temp dir and hand to the default app (PDFKit /
    /// AVFoundation open these; doc 08 §2.3).
    private func openExternally(_ rec: ResultRecord) {
        guard let path = try? results.session.extractTemp(id: rec.id) else { return }
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
    }
}
