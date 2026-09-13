import SwiftUI
import ReclaimCore
import ReclaimKit

// MARK: - Filter rail (doc 08 §2.3)

struct FilterRail: View {
    @ObservedObject var results: ResultsModel
    @FocusState private var searchFocused: Bool

    var body: some View {
        Form {
            Section("Search") {
                TextField("Path contains…", text: Binding(
                    get: { results.filter.pathSearch ?? "" },
                    set: { results.filter.pathSearch = $0.isEmpty ? nil : $0 }
                ))
                .focused($searchFocused)
                .onReceive(NotificationCenter.default.publisher(for: AppAction.focusFilter)) { _ in
                    searchFocused = true
                }
            }
            Section("Family") {
                Button {
                    results.filter.family = nil
                } label: {
                    HStack {
                        Text("All").foregroundStyle(results.filter.family == nil ? .primary : .secondary)
                        Spacer()
                    }
                }.buttonStyle(.plain)
                ForEach(results.families, id: \.family) { fc in
                    Button {
                        results.filter.family = fc.family
                    } label: {
                        HStack {
                            Image(systemName: symbol(fc.family))
                            Text(fc.family.capitalized)
                                .fontWeight(results.filter.family == fc.family ? .bold : .regular)
                            Spacer()
                            Text("\(fc.count)").foregroundStyle(.secondary).monospacedDigit()
                        }
                    }.buttonStyle(.plain)
                }
            }
            Section("Recoverability") {
                Picker("Chances", selection: $results.filter.recoverability) {
                    ForEach(Recoverability.allCases, id: \.self) { Text($0.label).tag($0) }
                }.labelsHidden().pickerStyle(.segmented)
            }
            Section("Engine") {
                Picker("Engine", selection: $results.filter.sourceKind) {
                    ForEach(SourceKindFilter.allCases, id: \.self) { Text($0.label).tag($0) }
                }.labelsHidden()
            }
            Section("Filters") {
                Toggle("Deleted only", isOn: $results.filter.deletedOnly)
                Toggle("Valid (full) only", isOn: $results.filter.fullOnly)
            }
            Section("Sort") {
                Picker("Sort", selection: $results.filter.sort) {
                    ForEach(ResultSort.allCases, id: \.self) { Text($0.label).tag($0) }
                }.labelsHidden()
            }
        }
        .formStyle(.grouped)
    }

    private func symbol(_ family: String) -> String {
        switch family {
        case "image", "raw": return "photo"
        case "video": return "film"
        case "audio": return "music.note"
        case "doc": return "doc.text"
        case "archive": return "archivebox"
        default: return "doc"
        }
    }
}

// MARK: - Grid (doc 08 §2.3)

struct ResultGrid: View {
    @ObservedObject var results: ResultsModel
    private let columns = [GridItem(.adaptive(minimum: 140), spacing: 10)]

    var body: some View {
        ScrollView {
            LazyVGrid(columns: columns, spacing: 10) {
                ForEach(results.page, id: \.id) { rec in
                    ThumbCell(results: results, rec: rec)
                }
            }
            .padding(10)
        }
    }
}

/// A grid cell with a lazily decoded thumbnail (off-main, doc 08 §4).
struct ThumbCell: View {
    @ObservedObject var results: ResultsModel
    let rec: ResultRecord
    @State private var image: NSImage?
    @State private var tried = false

    var body: some View {
        VStack(spacing: 4) {
            ZStack {
                RoundedRectangle(cornerRadius: 6).fill(Color.secondary.opacity(0.12))
                if let image {
                    Image(nsImage: image).resizable().scaledToFit().padding(4)
                } else {
                    Image(systemName: placeholderSymbol).font(.title).foregroundStyle(.secondary)
                }
                if results.selection.contains(rec.id) {
                    RoundedRectangle(cornerRadius: 6).strokeBorder(Color.accentColor, lineWidth: 3)
                }
                if rec.validity != "full" {
                    badge
                }
            }
            .frame(height: 110)
            Text(rec.path.split(separator: "/").last.map(String.init) ?? rec.path)
                .font(.caption).lineLimit(1).truncationMode(.middle)
        }
        .contentShape(Rectangle())
        .onTapGesture {
            results.focused = rec.id
            results.toggle(rec.id)
        }
        .help("\(rec.path) · \(rec.format) · \(formatBytes(rec.len))")
        .accessibilityLabel("\(rec.path), \(rec.format), \(formatBytes(rec.len))")
        .task(id: rec.id) { await loadThumb() }
    }

    private var badge: some View {
        VStack { HStack { Spacer()
            Text(rec.validity).font(.caption2.bold()).padding(2)
                .background(.orange.opacity(0.85), in: Capsule()).foregroundStyle(.white)
        }; Spacer() }.padding(4)
    }
    private var placeholderSymbol: String {
        switch rec.family {
        case "image", "raw": return "photo"
        case "video": return "film"
        case "audio": return "music.note"
        default: return "doc"
        }
    }
    private func loadThumb() async {
        guard !tried, rec.family == "image" || rec.family == "raw" else { return }
        tried = true
        let id = rec.id
        let session = results.session
        let data: Data? = await Task.detached {
            (try? session.preview(id: id, kind: .thumbnail, maxPx: 256)).map { Data($0.bytes) }
        }.value
        if let data { self.image = NSImage(data: data) }
    }
}

// MARK: - List (doc 08 §2.3)

struct ResultList: View {
    @ObservedObject var results: ResultsModel
    var body: some View {
        List {
            HStack {
                Text("Name").frame(maxWidth: .infinity, alignment: .leading)
                Text("Size").frame(width: 70, alignment: .trailing)
                Text("Date").frame(width: 90, alignment: .leading)
                Text("Chances").frame(width: 60, alignment: .trailing)
                Text("Engine").frame(width: 90, alignment: .leading)
                Text("").frame(width: 24)
            }
            .font(.caption.bold()).foregroundStyle(.secondary)
            ForEach(results.page, id: \.id) { r in
                ResultListRow(results: results, rec: r)
            }
        }
        .listStyle(.inset)
    }
}

private struct ResultListRow: View {
    @ObservedObject var results: ResultsModel
    let rec: ResultRecord
    var body: some View {
        let name = rec.path.split(separator: "/").last.map(String.init) ?? rec.path
        HStack {
            VStack(alignment: .leading, spacing: 1) {
                Text(name).lineLimit(1)
                Text(rec.path).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }.frame(maxWidth: .infinity, alignment: .leading)
            Text(formatBytes(rec.len)).frame(width: 70, alignment: .trailing).monospacedDigit()
            Text(rec.date ?? "—").frame(width: 90, alignment: .leading)
            Text("\(rec.score)").frame(width: 60, alignment: .trailing).monospacedDigit()
            Text(rec.sourceKind).frame(width: 90, alignment: .leading).font(.caption)
            Toggle("", isOn: Binding(
                get: { results.selection.contains(rec.id) },
                set: { _ in results.toggle(rec.id) }
            )).labelsHidden().frame(width: 24)
        }
        .background(results.focused == rec.id ? Color.accentColor.opacity(0.12) : .clear)
        .contentShape(Rectangle())
        .onTapGesture { results.focused = rec.id }
    }
}

// MARK: - Tree (doc 08 §2.3): group by top-level folder / reconstructed bucket

struct ResultTree: View {
    @ObservedObject var results: ResultsModel
    var body: some View {
        List {
            ForEach(groups.keys.sorted(), id: \.self) { key in
                DisclosureGroup("\(key) (\(groups[key]?.count ?? 0))") {
                    ForEach(groups[key] ?? [], id: \.id) { r in
                        HStack {
                            Image(systemName: r.sourceKind == "carved" ? "square.dashed" : "doc")
                            Text(r.path).lineLimit(1).truncationMode(.middle)
                            Spacer()
                            Text(formatBytes(r.len)).foregroundStyle(.secondary).monospacedDigit()
                            Toggle("", isOn: Binding(
                                get: { results.selection.contains(r.id) },
                                set: { _ in results.toggle(r.id) }
                            )).labelsHidden()
                        }
                        .contentShape(Rectangle())
                        .onTapGesture { results.focused = r.id }
                    }
                }
            }
        }
    }
    private var groups: [String: [ResultRecord]] {
        Dictionary(grouping: results.page) { rec in
            rec.sourceKind == "carved"
                ? "Reconstructed/\(rec.family)"
                : (rec.path.split(separator: "/").first.map(String.init) ?? "/")
        }
    }
}
