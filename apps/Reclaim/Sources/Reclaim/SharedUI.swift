import SwiftUI
import ReclaimCore
import ReclaimKit

/// The green/amber/red health chip (doc 08 §2.2).
struct HealthChip: View {
    let level: HealthLevel
    var body: some View {
        Label(text, systemImage: symbol)
            .font(.caption.bold())
            .padding(.horizontal, 8).padding(.vertical, 3)
            .background(color.opacity(0.18), in: Capsule())
            .foregroundStyle(color)
    }
    private var text: String {
        switch level {
        case .good: return "Healthy"
        case .warn: return "Marginal"
        case .bad: return "Failing"
        case .unknown: return "Unknown"
        }
    }
    private var symbol: String {
        switch level {
        case .good: return "checkmark.seal.fill"
        case .warn: return "exclamationmark.triangle.fill"
        case .bad: return "xmark.octagon.fill"
        case .unknown: return "questionmark.circle"
        }
    }
    private var color: Color {
        switch level {
        case .good: return .green
        case .warn: return .orange
        case .bad: return .red
        case .unknown: return .secondary
        }
    }
}

/// A proportional partition-map bar coloured by filesystem (doc 08 §2.2).
struct PartitionBar: View {
    let partitions: [PartitionInfo]
    let total: UInt64
    @Binding var selected: Int?

    var body: some View {
        GeometryReader { geo in
            HStack(spacing: 1) {
                ForEach(Array(partitions.enumerated()), id: \.offset) { idx, p in
                    Rectangle()
                        .fill(color(for: p))
                        .frame(width: max(2, width(p, geo.size.width)))
                        .overlay(
                            Rectangle().strokeBorder(
                                selected == idx ? Color.primary : .clear, lineWidth: 2)
                        )
                        .onTapGesture { selected = idx }
                        .help("\(p.typeLabel) · \(formatBytes(p.len))")
                }
            }
        }
        .frame(height: 26)
        .clipShape(RoundedRectangle(cornerRadius: 5))
    }

    private func width(_ p: PartitionInfo, _ w: CGFloat) -> CGFloat {
        guard total > 0 else { return 0 }
        return CGFloat(Double(p.len) / Double(total)) * w
    }
    private func color(for p: PartitionInfo) -> Color {
        let l = p.typeLabel.lowercased()
        if l.contains("apfs") { return .blue }
        if l.contains("hfs") { return .purple }
        if l.contains("ntfs") { return .teal }
        if l.contains("fat") || l.contains("exfat") { return .green }
        if l.contains("ext") { return .orange }
        if l.contains("efi") { return .gray }
        return .secondary
    }
}

/// A small labelled key/value.
struct KV: View {
    let key: String
    let value: String
    var body: some View {
        HStack(alignment: .top) {
            Text(key).foregroundStyle(.secondary).frame(width: 120, alignment: .trailing)
            Text(value).textSelection(.enabled)
            Spacer()
        }
        .font(.callout)
    }
}
