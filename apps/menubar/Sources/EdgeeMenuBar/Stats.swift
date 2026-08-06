import Foundation

/// Decoded from `edgee stats --json` (`StatsJson` in crates/cli/src/commands/stats.rs).
/// Field names come across via `.convertFromSnakeCase` (see EdgeeCLI) — no CodingKeys.
struct Stats: Codable {
    let sessions: Int
    let totals: Totals
    let recent: [SessionBrief]

    struct Totals: Codable {
        let requests: UInt64
        let errors: UInt64
        let inputTokens: UInt64
        let outputTokens: UInt64
        let tokenCostSavings: UInt64
        let uncompressedToolsTokens: UInt64
        let compressedToolsTokens: UInt64
        let compressionPct: UInt64?
    }

    struct SessionBrief: Codable, Identifiable {
        let sessionId: String
        let toolName: String
        let endedAt: String
        let endedAtUnix: Int64
        let requests: UInt64
        let inputTokens: UInt64
        let outputTokens: UInt64
        let errors: UInt64
        let compressionPct: UInt64?
        let logsUrl: String

        var id: String { sessionId }
    }
}

enum TokenFormat {
    /// Compact token count: 61410 → "61k", 1681 → "1.7k", 2_500_000 → "2.5M".
    static func short(_ value: UInt64) -> String {
        let v = Double(value)
        switch value {
        case 1_000_000...:
            return trim(v / 1_000_000, suffix: "M")
        case 1_000...:
            return trim(v / 1_000, suffix: "k")
        default:
            return "\(value)"
        }
    }

    private static func trim(_ value: Double, suffix: String) -> String {
        if value >= 100 || value == value.rounded() {
            return "\(Int(value.rounded()))\(suffix)"
        }
        return String(format: "%.1f%@", value, suffix)
    }
}
