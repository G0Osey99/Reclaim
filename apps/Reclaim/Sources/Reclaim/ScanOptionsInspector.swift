import SwiftUI
import ReclaimCore
import ReclaimKit

/// The docked, collapsible **Scan Options** inspector (GUI Option B, doc 08 §2.2).
/// Disclosure groups — Engines, Families, Range, Performance, Safety — each bound
/// to the real `ScanOptions` through `ScanPlanModel`. Editing anything re-derives
/// the plan's preset to Custom (handled in the model).
struct ScanOptionsInspector: View {
    @ObservedObject var plan: ScanPlanModel
    let probe: SourceProbe?

    @State private var showEngines = true
    @State private var showFamilies = true
    @State private var showRange = false
    @State private var showPerformance = false
    @State private var showSafety = true
    @State private var rangeCustom = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Scan Options").font(.headline)
                Spacer()
                Button("Reset") { plan.reset() }
                    .controlSize(.small)
                    .help("Reset to the Full preset")
            }
            .padding(.horizontal, 12)
            .padding(.top, 10)
            .padding(.bottom, 6)
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    enginesGroup
                    Divider()
                    familiesGroup
                    Divider()
                    rangeGroup
                    Divider()
                    performanceGroup
                    Divider()
                    safetyGroup
                }
                .padding(.horizontal, 12)
            }

            Divider()
            Text("Changing any option switches the preset to Custom. Options apply to this scan only.")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
        }
        .background(.background)
        .onAppear {
            rangeCustom = plan.options.rangeStart != nil || plan.options.rangeEnd != nil
        }
        .onChange(of: plan.preset) { p in
            if p != .custom { rangeCustom = false }
        }
    }

    // MARK: Engines

    private var enginesGroup: some View {
        DisclosureGroup(isExpanded: $showEngines) {
            VStack(alignment: .leading, spacing: 6) {
                switchRow(
                    "Filesystem metadata",
                    help: "Recover named files and recent deletions from the filesystem.",
                    isOn: bind(\.quick))
                switchRow(
                    "Signature carve",
                    help: "Reconstruct lost files from free space by content signature.",
                    isOn: bind(\.deep))
                if let autoText {
                    Label(autoText, systemImage: "wand.and.stars")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.top, 2)
                }
            }
            .padding(.vertical, 6)
        } label: {
            groupLabel("Engines", summary: enginesSummary)
        }
    }

    private var enginesSummary: String {
        switch (plan.options.quick, plan.options.deep) {
        case (true, true): return "Metadata + carve"
        case (true, false): return "Metadata"
        case (false, true): return "Carve"
        default: return "None"
        }
    }

    /// A read-only line naming the filesystem engines the planner auto-selects for
    /// this source, from the probed partition types.
    private var autoText: String? {
        guard let probe else { return nil }
        var kinds: [String] = []
        for p in probe.partitions {
            let l = p.typeLabel.lowercased()
            if l.contains("apfs") { kinds.append("APFS") }
            else if l.contains("hfs") { kinds.append("HFS+") }
            else if l.contains("ntfs") { kinds.append("NTFS") }
            else if l.contains("exfat") { kinds.append("exFAT") }
            else if l.contains("fat") { kinds.append("FAT") }
            else if l.contains("ext") { kinds.append("ext") }
            else if l.contains("iso") { kinds.append("ISO") }
        }
        var seen = Set<String>()
        let uniq = kinds.filter { seen.insert($0).inserted }
        var parts = uniq
        parts.append("lost-structure")
        return "Auto-selected for this source: \(parts.joined(separator: " · "))."
    }

    // MARK: Families

    private var familiesGroup: some View {
        DisclosureGroup(isExpanded: $showFamilies) {
            VStack(alignment: .leading, spacing: 8) {
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 86), spacing: 6)],
                    alignment: .leading, spacing: 6
                ) {
                    ForEach(ScanFamilyGroup.all) { g in
                        FamilyChip(
                            group: g,
                            on: plan.isFamilyGroupOn(g.id),
                            action: { plan.toggleFamilyGroup(g.id) })
                    }
                }
                Text("Restricts the carve pass. Filesystem metadata always recovers all types.")
                    .font(.caption2).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.vertical, 6)
        } label: {
            groupLabel("Families", summary: familiesSummary)
        }
    }

    private var familiesSummary: String {
        plan.options.families.isEmpty ? "All" : "\(plan.selectedFamilyGroups.count) of \(ScanFamilyGroup.all.count)"
    }

    // MARK: Range

    private var rangeGroup: some View {
        DisclosureGroup(isExpanded: $showRange) {
            VStack(alignment: .leading, spacing: 8) {
                Picker("Scope", selection: Binding(
                    get: { rangeCustom },
                    set: { on in
                        rangeCustom = on
                        if !on { plan.edit { $0.rangeStart = nil; $0.rangeEnd = nil } }
                    }
                )) {
                    Text("Whole source").tag(false)
                    Text("Custom range").tag(true)
                }
                .pickerStyle(.radioGroup)
                .labelsHidden()

                if rangeCustom {
                    HStack {
                        Text("Start").foregroundStyle(.secondary).frame(width: 44, alignment: .trailing)
                        TextField("0", text: byteText(\.rangeStart))
                            .textFieldStyle(.roundedBorder).frame(width: 130)
                        Text("bytes").font(.caption).foregroundStyle(.secondary)
                    }
                    HStack {
                        Text("End").foregroundStyle(.secondary).frame(width: 44, alignment: .trailing)
                        TextField("end of source", text: byteText(\.rangeEnd))
                            .textFieldStyle(.roundedBorder).frame(width: 130)
                        Text("bytes").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
            .padding(.vertical, 6)
        } label: {
            groupLabel("Range", summary: rangeCustom ? "Custom" : "Whole source")
        }
    }

    // MARK: Performance

    private var performanceGroup: some View {
        DisclosureGroup(isExpanded: $showPerformance) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("Block size").frame(width: 92, alignment: .leading)
                    Spacer()
                    Picker("", selection: bind(\.blockSize)) {
                        ForEach(ScanBlockSize.options, id: \.value) { Text($0.label).tag($0.value) }
                    }.labelsHidden().fixedSize()
                }
                HStack {
                    Text("Checkpoint").frame(width: 92, alignment: .leading)
                    Spacer()
                    Picker("", selection: bind(\.checkpointSecs)) {
                        ForEach(ScanCheckpoint.options, id: \.value) { Text($0.label).tag($0.value) }
                    }.labelsHidden().fixedSize()
                }
                Text("Block size Auto lets the planner infer it. Sessions auto-save at the checkpoint interval.")
                    .font(.caption2).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.vertical, 6)
        } label: {
            groupLabel(
                "Performance",
                summary: "\(ScanBlockSize.label(plan.options.blockSize)) · \(ScanCheckpoint.label(plan.options.checkpointSecs))")
        }
    }

    // MARK: Safety

    private var safetyGroup: some View {
        DisclosureGroup(isExpanded: $showSafety) {
            VStack(alignment: .leading, spacing: 6) {
                switchRow(
                    "Keep corrupted / partial",
                    help: "Show truncated or suspect carved files (badged in results).",
                    isOn: bind(\.keepCorrupted))
                switchRow(
                    "Unallocated space only",
                    help: "Skip blocks the filesystem still owns. Faster on nearly-full drives.",
                    isOn: bind(\.unallocatedOnly))
                switchRow(
                    "Brute-force fragmented",
                    help: "Byte-granular matching. Much slower; only for badly fragmented files.",
                    isOn: bind(\.bruteForce))
                HStack {
                    Text("Max file size").frame(width: 100, alignment: .leading)
                    Spacer()
                    Picker("", selection: bind(\.maxFileSize)) {
                        ForEach(ScanMaxFileSize.options, id: \.value) { Text($0.label).tag($0.value) }
                    }.labelsHidden().fixedSize()
                }
            }
            .padding(.vertical, 6)
        } label: {
            groupLabel("Safety", summary: plan.options.keepCorrupted ? "Keeps partials" : "Full only")
        }
    }

    // MARK: Helpers

    private func groupLabel(_ title: String, summary: String) -> some View {
        HStack {
            Text(title).font(.subheadline.weight(.semibold))
            Spacer()
            Text(summary).font(.caption).foregroundStyle(.secondary)
        }
    }

    private func switchRow(_ title: String, help: String, isOn: Binding<Bool>) -> some View {
        Toggle(isOn: isOn) {
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                Text(help).font(.caption2).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .toggleStyle(.switch)
        .controlSize(.small)
    }

    /// A binding into `plan.options` that routes writes through `plan.edit`, so
    /// every change re-derives the preset.
    private func bind<V>(_ keyPath: WritableKeyPath<ScanOptions, V>) -> Binding<V> {
        Binding(
            get: { plan.options[keyPath: keyPath] },
            set: { newValue in plan.edit { $0[keyPath: keyPath] = newValue } })
    }

    /// A string binding for an optional byte count (empty / invalid → nil).
    private func byteText(_ keyPath: WritableKeyPath<ScanOptions, UInt64?>) -> Binding<String> {
        Binding(
            get: { plan.options[keyPath: keyPath].map(String.init) ?? "" },
            set: { s in
                let trimmed = s.trimmingCharacters(in: .whitespaces)
                plan.edit { $0[keyPath: keyPath] = trimmed.isEmpty ? nil : UInt64(trimmed) }
            })
    }
}

/// One selectable family chip in the inspector.
private struct FamilyChip: View {
    let group: ScanFamilyGroup
    let on: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                Image(systemName: group.symbol)
                Text(group.label).lineLimit(1)
            }
            .font(.caption)
            .padding(.horizontal, 9).padding(.vertical, 4)
            .frame(maxWidth: .infinity)
            .background(
                on ? Color.accentColor : Color.secondary.opacity(0.14),
                in: Capsule())
            .foregroundStyle(on ? Color.white : Color.primary)
        }
        .buttonStyle(.plain)
        .help(on ? "Included in carving" : "Excluded from carving")
        .accessibilityLabel("\(group.label), \(on ? "included" : "excluded")")
    }
}
