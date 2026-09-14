import SwiftUI
import AppKit
import ReclaimCore
import ReclaimKit

/// The recover sheet (doc 08 §2.4): destination + validation + options + run.
struct RecoverSheet: View {
    @ObservedObject var results: ResultsModel
    @Environment(\.dismiss) private var dismiss

    @State private var dest: String = ""
    @State private var preservePaths = true
    @State private var flat = false
    @State private var collision: CollisionPolicy = .rename
    @State private var verify = true
    @State private var check: DestinationCheck?
    @State private var running = false
    @State private var progress: (done: UInt64, total: UInt64) = (0, 0)
    @State private var outcome: RecoverOutcome?
    @State private var errorMessage: String?

    private var verdict: DestinationVerdict? {
        check.map { evaluateDestination($0, allowSameDevice: false) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Recover \(results.selectedCount) item\(results.selectedCount == 1 ? "" : "s") · \(formatBytes(results.selectedBytes))")
                .font(.title3.bold())

            if let outcome {
                summaryView(outcome)
            } else {
                destinationRow
                if let v = verdict {
                    if let reason = v.blockingReason {
                        Label(reason, systemImage: "xmark.octagon.fill")
                            .foregroundStyle(.red).font(.callout)
                    } else if let warn = v.warning {
                        Label(warn, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange).font(.callout)
                    } else if let free = check?.freeBytes {
                        Label("\(formatBytes(free)) free on destination", systemImage: "checkmark.circle")
                            .foregroundStyle(.green).font(.callout)
                    }
                }
                optionsBox
                if let e = errorMessage {
                    Label(e, systemImage: "xmark.octagon").foregroundStyle(.red).font(.callout)
                }
                if running {
                    ProgressView(
                        value: Double(progress.done),
                        total: Double(max(progress.total, 1))
                    )
                    Text("Recovering \(progress.done)/\(progress.total)…")
                        .font(.caption).foregroundStyle(.secondary)
                }
                controls
            }
        }
        .padding(20)
        .frame(width: 520)
    }

    private var destinationRow: some View {
        HStack {
            TextField("Destination folder", text: $dest)
                .textFieldStyle(.roundedBorder)
                .onChange(of: dest) { _ in revalidate() }
            Button("Choose…") { pickFolder() }
        }
    }

    private var optionsBox: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 6) {
                Picker("Layout", selection: Binding(
                    get: { preservePaths && !flat },
                    set: { preservePaths = $0; flat = !$0 }
                )) {
                    Text("Preserve folder structure").tag(true)
                    Text("Flat, by name").tag(false)
                }.pickerStyle(.radioGroup)
                Divider()
                Picker("Name collisions", selection: $collision) {
                    Text("Rename").tag(CollisionPolicy.rename)
                    Text("Skip").tag(CollisionPolicy.skip)
                    Text("Overwrite").tag(CollisionPolicy.overwrite)
                }
                Toggle("Verify each file after copy (re-read + hash)", isOn: $verify)
            }.frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var controls: some View {
        HStack {
            Button("Cancel") { dismiss() }
            Spacer()
            Button {
                run()
            } label: {
                Label("Recover", systemImage: "square.and.arrow.down")
            }
            .buttonStyle(.borderedProminent)
            .disabled(dest.isEmpty || running || verdict?.canRecover != true)
        }
    }

    private func summaryView(_ o: RecoverOutcome) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Recovered \(o.recovered) file(s)", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green).font(.headline)
            if o.partial > 0 {
                Text("\(o.partial) partial/suspect").foregroundStyle(.orange)
            }
            if o.verifyFailed > 0 {
                Label("\(o.verifyFailed) failed verification", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
            } else if verify {
                Text("All files verified.").foregroundStyle(.secondary)
            }
            Text("\(formatBytes(o.bytes)) written to \(o.dest)").font(.callout).foregroundStyle(.secondary)
            HStack {
                Button("Open in Finder") {
                    NSWorkspace.shared.open(URL(fileURLWithPath: o.dest))
                }
                Button("Reveal manifest") {
                    NSWorkspace.shared.selectFile(o.manifestPath, inFileViewerRootedAtPath: o.dest)
                }
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }.padding(.top, 4)
        }
    }

    private func pickFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Choose"
        if panel.runModal() == .OK, let url = panel.url {
            dest = url.path
            revalidate()
        }
    }

    private func revalidate() {
        guard !dest.isEmpty else { check = nil; errorMessage = nil; return }
        do {
            check = try results.checkDestination(dest)
            errorMessage = nil
        } catch {
            check = nil
            errorMessage = "Destination check failed: \(error)"
        }
    }

    private func run() {
        guard verdict?.canRecover == true else { return }
        running = true
        errorMessage = nil
        let opts = Options.recover(
            dest: dest, preservePaths: preservePaths, flat: flat,
            collision: collision, verify: verify, allowSameDevice: false)
        let sink = EventForwarder { ev in
            if case let .recoverProgress(done, total, _) = ev {
                progress = (done, total)
            }
        }
        let session = results.session
        let ids = Array(results.selection)
        Task.detached {
            let result: Result<RecoverOutcome, Error>
            do { result = .success(try session.recover(ids: ids, opts: opts, sink: sink)) }
            catch { result = .failure(error) }
            await MainActor.run {
                running = false
                switch result {
                case .success(let o): outcome = o
                case .failure(let e): errorMessage = "\(e)"
                }
            }
        }
    }
}
