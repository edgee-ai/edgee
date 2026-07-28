import AppKit
import SwiftUI

/// The stats section: KPI tiles plus a compact recent-sessions list. Fed by
/// `edgee stats --json`.
struct StatsView: View {
    let stats: Stats?
    let loading: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Label("Stats", systemImage: "chart.bar.fill").font(.headline)
                Spacer()
                if let stats {
                    Text("\(stats.sessions) sessions")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }

            if stats == nil && loading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading…").foregroundStyle(.secondary)
                }
            } else if let stats, stats.sessions > 0 {
                tiles(stats.totals)
                if !stats.recent.isEmpty {
                    VStack(alignment: .leading, spacing: 3) {
                        ForEach(stats.recent.prefix(5)) { sessionRow($0) }
                    }
                    .padding(.top, 2)
                }
            } else {
                Text("No sessions yet. Launch an agent through Edgee to see stats.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func tiles(_ totals: Stats.Totals) -> some View {
        HStack(spacing: 6) {
            tile("Requests", "\(totals.requests)")
            tile("In", TokenFormat.short(totals.inputTokens))
            tile("Out", TokenFormat.short(totals.outputTokens))
            tile("Comp.", totals.compressionPct.map { "\($0)%" } ?? "—")
        }
    }

    private func tile(_ label: String, _ value: String) -> some View {
        VStack(spacing: 2) {
            Text(value)
                .font(.system(.body, design: .rounded).weight(.semibold))
                .lineLimit(1)
                .minimumScaleFactor(0.6)
            Text(label).font(.caption2).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 8))
    }

    private func sessionRow(_ session: Stats.SessionBrief) -> some View {
        Button {
            if let url = URL(string: session.logsUrl) { NSWorkspace.shared.open(url) }
        } label: {
            HStack(spacing: 6) {
                Text(session.toolName)
                    .font(.caption)
                    .frame(width: 66, alignment: .leading)
                    .lineLimit(1)
                Text(relative(session.endedAtUnix))
                    .font(.caption2).foregroundStyle(.secondary)
                Spacer()
                Text("\(session.requests) req")
                    .font(.caption2).foregroundStyle(.secondary)
                Text("\(TokenFormat.short(session.inputTokens))→\(TokenFormat.short(session.outputTokens))")
                    .font(.caption2).foregroundStyle(.secondary)
                if let pct = session.compressionPct {
                    Text("\(pct)%").font(.caption2).foregroundStyle(.green)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Open session on the Edgee console")
    }

    private func relative(_ unix: Int64) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(
            for: Date(timeIntervalSince1970: TimeInterval(unix)), relativeTo: Date())
    }
}
