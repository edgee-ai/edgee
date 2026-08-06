import Foundation

/// A relay/launch target the menubar can drive via `edgee relay <id>`.
struct RelayTarget: Identifiable {
    /// The `edgee relay` argument (also the id).
    let id: String
    let name: String
    /// SF Symbol shown next to the row.
    let symbol: String
    /// Proxy-only (`--no-launch`) — for external clients like Claude Desktop.
    /// When false, the relay also launches the app.
    let proxyOnly: Bool
    /// App bundle paths used to detect installation. Empty = always available.
    let detectPaths: [String]

    var installed: Bool {
        if detectPaths.isEmpty { return true }
        return detectPaths.contains { FileManager.default.fileExists(atPath: $0) }
    }

    /// The installed app bundle path (first existing detectPath), used to render
    /// the app's real macOS icon. `nil` → fall back to the SF Symbol.
    var appBundlePath: String? {
        detectPaths.first { FileManager.default.fileExists(atPath: $0) }
    }

    private static func appPaths(_ name: String) -> [String] {
        [
            "/Applications/\(name)",
            "\(NSHomeDirectory())/Applications/\(name)",
        ]
    }

    /// The targets offered in the UI. Editors launch behind the relay; the
    /// proxy-only entry is for pointing an external app (Claude Desktop) at it.
    static let all: [RelayTarget] = [
        RelayTarget(
            id: "cursor", name: "Cursor", symbol: "cursorarrow.rays",
            proxyOnly: false, detectPaths: appPaths("Cursor.app")),
        RelayTarget(
            id: "copilot-vscode", name: "VS Code (Copilot)", symbol: "chevron.left.forwardslash.chevron.right",
            proxyOnly: false, detectPaths: appPaths("Visual Studio Code.app")),
        RelayTarget(
            id: "claude", name: "Claude Desktop", symbol: "network",
            proxyOnly: true, detectPaths: appPaths("Claude.app")),
    ]
}

enum RelayRunState: Equatable {
    case stopped
    case starting
    case running
    case failed(String)
}

/// Supervises `edgee relay` subprocesses — one per target — as headless
/// background processes (no shell/terminal). Owned at the App level so relays
/// survive the popover opening and closing.
@MainActor
final class RelayManager: ObservableObject {
    @Published private(set) var states: [String: RelayRunState] = [:]
    private var processes: [String: Process] = [:]

    func state(_ id: String) -> RelayRunState { states[id] ?? .stopped }

    func toggle(_ target: RelayTarget) {
        switch state(target.id) {
        case .running, .starting:
            stop(target.id)
        case .stopped, .failed:
            start(target)
        }
    }

    func start(_ target: RelayTarget) {
        guard processes[target.id] == nil else { return }
        states[target.id] = .starting

        var args = ["relay", target.id, "--non-interactive"]
        if target.proxyOnly { args.append("--no-launch") }

        guard let spawned = EdgeeCLI.spawn(args) else {
            states[target.id] = .failed("could not start edgee")
            return
        }
        processes[target.id] = spawned.process

        spawned.process.terminationHandler = { [weak self] proc in
            let errText = spawned.stderr.text
            Task { @MainActor in
                guard let self else { return }
                self.processes[target.id] = nil
                // A SIGINT/terminate from us is a clean stop; a non-zero exit we
                // didn't request is a failure worth surfacing.
                if proc.terminationReason == .uncaughtSignal || proc.terminationStatus == 0 {
                    self.states[target.id] = .stopped
                } else {
                    let lastLine =
                        errText.split(whereSeparator: \.isNewline).last.map(String.init)
                        ?? "relay exited (code \(proc.terminationStatus))"
                    self.states[target.id] = .failed(lastLine)
                }
            }
        }

        states[target.id] = .running
    }

    func stop(_ id: String) {
        // SIGINT triggers the relay's graceful shutdown; terminationHandler
        // flips the state back to .stopped.
        processes[id]?.interrupt()
    }

    func stopAll() {
        for process in processes.values {
            process.interrupt()
        }
    }
}
