import Foundation

/// Decoded from `edgee auth status --json`. Mirrors `AuthStatusJson` in the CLI
/// (crates/cli/src/commands/auth/status.rs).
struct AuthStatus: Codable {
    let loggedIn: Bool
    let profile: String
    let configPath: String
    let email: String?
    let orgSlug: String?
    let providers: [String: ProviderStatus]

    enum CodingKeys: String, CodingKey {
        case loggedIn = "logged_in"
        case profile
        case configPath = "config_path"
        case email
        case orgSlug = "org_slug"
        case providers
    }
}

struct ProviderStatus: Codable {
    let configured: Bool
    let mode: String?
}

/// Decoded from `edgee auth login --non-interactive --json` (`LoginOutcome` in
/// crates/cli/src/commands/auth/login.rs).
struct LoginOutcome: Codable {
    let loggedIn: Bool
    let email: String?
    let orgSlug: String?
    let needsOrgSelection: Bool

    enum CodingKeys: String, CodingKey {
        case loggedIn = "logged_in"
        case email
        case orgSlug = "org_slug"
        case needsOrgSelection = "needs_org_selection"
    }
}
