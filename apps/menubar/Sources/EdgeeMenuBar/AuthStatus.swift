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

/// One entry from `edgee auth list --json` (`ProfileEntry` in the CLI).
struct Profile: Codable, Identifiable {
    let name: String
    let active: Bool
    let email: String?
    let orgSlug: String?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, active, email
        case orgSlug = "org_slug"
    }
}

/// One entry from `edgee auth orgs --json` (`OrgEntry` in the CLI).
struct Org: Codable, Identifiable {
    let id: String
    let slug: String
    let name: String
    let active: Bool
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
