import SwiftUI
import ReclaimCore
import ReclaimKit

enum ViewMode: String, CaseIterable { case grid, list, tree }

/// The scan / results screen — results appear live (doc 08 §2.3).
struct ScanResultsView: View {
    @ObservedObject var scan: ScanController
    let session: Session
    let title: String

    @StateObject private var results: ResultsModel
    @State private var mode: ViewMode = .grid
    @State private var showRecover = false

    init(scan: ScanController, session: Session, title: String) {
        self.scan = scan
        self.session = session
        self.title = title
        _results = StateObject(wrappedValue: ResultsModel(session: session))
    }

    var body: some View {
        VStack(spacing: 0) {
            progressBar
            Divider()
            HSplitView {
                FilterRail(results: results)
                    .frame(minWidth: 200, maxWidth: 260)
                center
                    .frame(minWidth: 320)
                InspectorView(results: results)
                    .frame(minWidth: 260, maxWidth: 360)
            }
            Divider()
            footer
        }
        .navigationTitle(title)
        .task { await livePoll() }
        .onChange(of: results.filter) { _ in results.reload() }
        .onReceive(NotificationCenter.default.publisher(for: AppAction.stop)) { _ in scan.stop() }
        .onReceive(NotificationCenter.default.publisher(for: AppAction.recover)) { _ in
            if results.selectedCount > 0 { showRecover = true }
        }
        .sheet(isPresented: $showRecover) {
            RecoverSheet(results: results)
        }
    }

    // Live results: reload periodically while scanning, then once at the end.
    private func livePoll() async {
        results.reload()
        while scan.isScanning, !Task.isCancelled {
            do { try await Task.sleep(nanoseconds: 800_000_000) } catch { return }
            results.reload()
        }
        results.reload()
    }

    private var progressBar: some View {
        VStack(spacing: 6) {
            HStack(spacing: 12) {
                if scan.isScanning {
                    ProgressView(value: scan.pct, total: 100).frame(width: 200)
                    Text(String(format: "%.1f%%", scan.pct)).monospacedDigit()
                    Text("\(formatBytes(scan.rate))/s").foregroundStyle(.secondary).monospacedDigit()
                } else if let s = scan.summary {
                    Image(systemName: s.complete ? "checkmark.circle.fill" : "pause.circle")
                        .foregroundStyle(s.complete ? .green : .orange)
                    Text(s.complete ? "Scan complete" : "Paused")
                }
                Spacer()
                Text("\(scan.foundCount) found")
                if scan.readErrors > 0 {
                    Label("\(scan.readErrors)", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
                controlButtons
            }
            if scan.isScanning, !scan.pass.isEmpty {
                HStack {
                    Text("Pass: \(scan.pass)").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    if let cp = scan.lastCheckpoint {
                        Text("Session auto-saved \(relative(cp))")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
            if let e = scan.errorMessage {
                Label(e, systemImage: "ant").font(.caption).foregroundStyle(.orange)
            }
        }
        .padding(10)
    }

    @ViewBuilder private var controlButtons: some View {
        if scan.isScanning {
            Button { scan.pause() } label: { Image(systemName: "pause.fill") }
                .help("Pause (checkpoints; resumable)")
            Button { scan.stop() } label: { Image(systemName: "stop.fill") }
                .keyboardShortcut(".", modifiers: .command).help("Stop")
        } else if scan.paused {
            Button { scan.resume() } label: { Image(systemName: "play.fill") }
                .help("Resume")
        }
    }

    private var center: some View {
        VStack(spacing: 0) {
            HStack {
                Picker("", selection: $mode) {
                    Image(systemName: "square.grid.2x2").tag(ViewMode.grid)
                    Image(systemName: "list.bullet").tag(ViewMode.list)
                    Image(systemName: "list.bullet.indent").tag(ViewMode.tree)
                }
                .pickerStyle(.segmented).fixedSize()
                Spacer()
                Text("\(results.total) results").foregroundStyle(.secondary).font(.callout)
                pager
            }
            .padding(8)
            Divider()
            switch mode {
            case .grid: ResultGrid(results: results)
            case .list: ResultList(results: results)
            case .tree: ResultTree(results: results)
            }
        }
    }

    @ViewBuilder private var pager: some View {
        if results.pageCount > 1 {
            let current = Int(results.offset / max(results.pageSize, 1))
            HStack(spacing: 4) {
                Button { results.goToPage(current - 1) } label: { Image(systemName: "chevron.left") }
                    .disabled(current == 0)
                Text("\(current + 1)/\(results.pageCount)").font(.caption).monospacedDigit()
                Button { results.goToPage(current + 1) } label: { Image(systemName: "chevron.right") }
                    .disabled(current + 1 >= results.pageCount)
            }
        }
    }

    private var footer: some View {
        HStack {
            Button(results.selectedCount == 0 ? "Select all on page" : "Clear selection") {
                if results.selectedCount == 0 { results.selectAllOnPage() } else { results.clearSelection() }
            }
            Spacer()
            if results.selectedCount > 0 {
                Text("\(results.selectedCount) items · \(formatBytes(results.selectedBytes))")
                    .foregroundStyle(.secondary)
            }
            Button {
                showRecover = true
            } label: {
                Label("Recover…", systemImage: "square.and.arrow.down")
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut("r", modifiers: .command)
            .disabled(results.selectedCount == 0)
        }
        .padding(10)
    }

    private func relative(_ d: Date) -> String {
        let s = Int(Date().timeIntervalSince(d))
        return s <= 1 ? "just now" : "\(s)s ago"
    }
}

/// Opens an existing session for browsing (sidebar "Sessions").
struct SessionResultsLoader: View {
    @EnvironmentObject var model: AppModel
    let sessionDir: String
    @State private var session: Session?
    @State private var error: String?

    var body: some View {
        Group {
            if let session {
                BrowseResultsView(session: session, title: (sessionDir as NSString).lastPathComponent)
            } else if let error {
                ContentUnavailableCompat(title: "Cannot open session", detail: error)
            } else {
                ProgressView().onAppear(perform: load)
            }
        }
    }
    private func load() {
        do { session = try openSession(dir: sessionDir) } catch { self.error = "\(error)" }
    }
}

/// Results browser for a static (finished/reopened) session — no scan controls.
struct BrowseResultsView: View {
    let session: Session
    let title: String
    @StateObject private var results: ResultsModel
    @State private var mode: ViewMode = .grid
    @State private var showRecover = false

    init(session: Session, title: String) {
        self.session = session
        self.title = title
        _results = StateObject(wrappedValue: ResultsModel(session: session))
    }

    var body: some View {
        VStack(spacing: 0) {
            HSplitView {
                FilterRail(results: results).frame(minWidth: 200, maxWidth: 260)
                VStack(spacing: 0) {
                    HStack {
                        Picker("", selection: $mode) {
                            Image(systemName: "square.grid.2x2").tag(ViewMode.grid)
                            Image(systemName: "list.bullet").tag(ViewMode.list)
                            Image(systemName: "list.bullet.indent").tag(ViewMode.tree)
                        }.pickerStyle(.segmented).fixedSize()
                        Spacer()
                        Text("\(results.total) results").foregroundStyle(.secondary)
                    }.padding(8)
                    Divider()
                    switch mode {
                    case .grid: ResultGrid(results: results)
                    case .list: ResultList(results: results)
                    case .tree: ResultTree(results: results)
                    }
                }.frame(minWidth: 320)
                InspectorView(results: results).frame(minWidth: 260, maxWidth: 360)
            }
            Divider()
            HStack {
                Spacer()
                if results.selectedCount > 0 {
                    Text("\(results.selectedCount) · \(formatBytes(results.selectedBytes))")
                        .foregroundStyle(.secondary)
                }
                Button { showRecover = true } label: { Label("Recover…", systemImage: "square.and.arrow.down") }
                    .buttonStyle(.borderedProminent).disabled(results.selectedCount == 0)
            }.padding(10)
        }
        .navigationTitle(title)
        .onAppear { results.reload() }
        .onChange(of: results.filter) { _ in results.reload() }
        .sheet(isPresented: $showRecover) { RecoverSheet(results: results) }
    }
}

/// A tiny compatibility stand-in for ContentUnavailableView (macOS 13).
struct ContentUnavailableCompat: View {
    let title: String
    let detail: String
    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle").font(.largeTitle).foregroundStyle(.secondary)
            Text(title).font(.headline)
            Text(detail).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }.frame(maxWidth: .infinity, maxHeight: .infinity).padding()
    }
}
