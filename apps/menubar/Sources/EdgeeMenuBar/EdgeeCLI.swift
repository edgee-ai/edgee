import Foundation

/// Thin wrapper for shelling out to the `edgee` binary.
///
/// GUI apps don't inherit the user's shell `PATH`, so we resolve the binary from
/// a set of well-known locations and also augment `PATH` for any tools `edgee`
/// itself spawns. Override with the `EDGEE_BIN` environment variable.
enum EdgeeCLI {
    /// Absolute path to the edgee binary. Preference order:
    /// 1. `EDGEE_BIN` override, 2. the copy bundled inside Edgee.app (so the app
    /// always runs a CLI that matches its own version), 3. well-known install dirs.
    private static let resolvedBinary: String? = {
        let env = ProcessInfo.processInfo.environment
        if let override = env["EDGEE_BIN"], !override.isEmpty {
            return override
        }
        if let bundled = Bundle.main.resourceURL?.appendingPathComponent("edgee").path,
            FileManager.default.isExecutableFile(atPath: bundled)
        {
            return bundled
        }
        return searchDirs
            .map { "\($0)/edgee" }
            .first { FileManager.default.isExecutableFile(atPath: $0) }
    }()

    /// Directories prepended to `PATH` (and scanned for the binary).
    private static let searchDirs = [
        "\(NSHomeDirectory())/.local/bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
    ]

    /// Fetch auth status via `edgee auth status --json`. Runs off the main actor.
    static func authStatus() async -> AuthStatus? {
        await Task.detached(priority: .userInitiated) {
            guard let data = capture(["auth", "status", "--json"]) else { return nil }
            return try? JSONDecoder().decode(AuthStatus.self, from: data)
        }.value
    }

    /// Fetch aggregated stats via `edgee stats --json`. Runs off the main actor.
    static func stats(limit: Int = 20) async -> Stats? {
        await Task.detached(priority: .userInitiated) {
            guard let data = capture(["stats", "--json", "--limit", "\(limit)"]) else { return nil }
            return try? JSONDecoder().decode(Stats.self, from: data)
        }.value
    }

    /// List configured profiles via `edgee auth list --json`.
    static func profiles() async -> [Profile] {
        await Task.detached(priority: .userInitiated) {
            guard let data = capture(["auth", "list", "--json"]) else { return [] }
            return (try? JSONDecoder().decode([Profile].self, from: data)) ?? []
        }.value
    }

    /// Switch the active profile via `edgee auth switch <name>`. Returns success.
    @discardableResult
    static func switchProfile(_ name: String) async -> Bool {
        await Task.detached(priority: .userInitiated) {
            capture(["auth", "switch", name]) != nil
        }.value
    }

    /// List the account's organizations via `edgee auth orgs --json` (network).
    static func orgs() async -> [Org] {
        await Task.detached(priority: .userInitiated) {
            guard let data = capture(["auth", "orgs", "--json"]) else { return [] }
            return (try? JSONDecoder().decode([Org].self, from: data)) ?? []
        }.value
    }

    /// Switch the active profile's org via `edgee auth orgs --set <id|slug>`.
    @discardableResult
    static func switchOrg(_ idOrSlug: String) async -> Bool {
        await Task.detached(priority: .userInitiated) {
            capture(["auth", "orgs", "--set", idOrSlug]) != nil
        }.value
    }

    /// Run the browser login headlessly (no Terminal): `edgee auth login
    /// --non-interactive --json`. The subprocess opens the browser itself and
    /// blocks until the callback arrives, so this can take a while — run it off
    /// the main actor. Returns the decoded outcome, or nil on failure.
    static func login() async -> LoginOutcome? {
        await Task.detached(priority: .userInitiated) {
            guard let data = capture(["auth", "login", "--non-interactive", "--json"]) else {
                return nil
            }
            return try? JSONDecoder().decode(LoginOutcome.self, from: data)
        }.value
    }

    /// Run edgee and capture stdout. Returns nil on launch failure or non-zero exit.
    /// Configure a `Process` to invoke edgee with an augmented PATH (so edgee and
    /// any editors it launches resolve), without running it.
    private static func makeProcess(_ args: [String]) -> Process {
        let process = Process()

        var environment = ProcessInfo.processInfo.environment
        let existingPath = environment["PATH"] ?? ""
        environment["PATH"] = (searchDirs + [existingPath]).joined(separator: ":")
        process.environment = environment

        if let bin = resolvedBinary {
            process.executableURL = URL(fileURLWithPath: bin)
            process.arguments = args
        } else {
            // Last resort: let `env` resolve `edgee` off the augmented PATH.
            process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
            process.arguments = ["edgee"] + args
        }
        return process
    }

    private static func capture(_ args: [String]) -> Data? {
        let process = makeProcess(args)
        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()

        do {
            try process.run()
        } catch {
            return nil
        }
        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        return data
    }

    /// Spawn a long-running edgee subprocess (e.g. a relay) in the background —
    /// no shell, no window. Returns the running process plus a pipe capturing its
    /// stderr (for diagnostics on exit), or nil if it couldn't be launched.
    static func spawn(_ args: [String]) -> (process: Process, stderr: Pipe)? {
        let process = makeProcess(args)
        let stderr = Pipe()
        process.standardOutput = Pipe()
        process.standardError = stderr

        do {
            try process.run()
        } catch {
            return nil
        }
        return (process, stderr)
    }
}
