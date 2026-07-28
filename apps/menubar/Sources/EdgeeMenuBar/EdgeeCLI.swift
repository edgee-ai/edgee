import Foundation

/// Thin wrapper for shelling out to the `edgee` binary.
///
/// GUI apps don't inherit the user's shell `PATH`, so we resolve the binary from
/// a set of well-known locations and also augment `PATH` for any tools `edgee`
/// itself spawns. Override with the `EDGEE_BIN` environment variable.
enum EdgeeCLI {
    /// Absolute path to the edgee binary, if found in a known location.
    private static let resolvedBinary: String? = {
        let env = ProcessInfo.processInfo.environment
        if let override = env["EDGEE_BIN"], !override.isEmpty {
            return override
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
    private static func capture(_ args: [String]) -> Data? {
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
}
