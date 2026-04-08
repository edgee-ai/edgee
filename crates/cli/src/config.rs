use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct ProviderConfig {
    pub api_key: String,
    pub api_key_id: Option<String>,
    pub connection: Option<String>, // "plan" | "api"
}

const CURRENT_VERSION: u32 = 4;

/// Per-profile credentials (formerly the flat `Credentials` struct).
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Profile {
    pub user_token: Option<String>,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub org_slug: Option<String>,
    pub org_id: Option<String>,
    /// Override for EDGEE_CONSOLE_URL (e.g. http://localhost:3000)
    pub console_url: Option<String>,
    /// Override for EDGEE_CONSOLE_API_URL (e.g. http://localhost:4000)
    pub console_api_url: Option<String>,
    /// Override for EDGEE_API_URL / gateway (e.g. http://localhost:5000)
    pub gateway_url: Option<String>,
    pub claude: Option<ProviderConfig>,
    pub codex: Option<ProviderConfig>,
    pub opencode: Option<ProviderConfig>,
}

/// Type alias so existing call sites that use `Credentials` compile unchanged.
pub type Credentials = Profile;

/// Top-level structure of the credentials.toml file (v4).
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct CredentialsFile {
    pub version: Option<u32>,
    pub active_profile: Option<String>,
    pub profiles: std::collections::BTreeMap<String, Profile>,
}

// ---------------------------------------------------------------------------
// Process-level active profile (set once in main(), read everywhere else).
// ---------------------------------------------------------------------------

static ACTIVE_PROFILE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_active_profile(name: String) {
    let _ = ACTIVE_PROFILE.set(name);
}

pub fn active_profile_name() -> String {
    ACTIVE_PROFILE
        .get()
        .cloned()
        .unwrap_or_else(|| "default".to_string())
}

// ---------------------------------------------------------------------------
// File paths
// ---------------------------------------------------------------------------

/// Returns the project-local credentials path if `.edgee/credentials.toml` exists
/// in the current working directory.
pub fn local_credentials_path() -> Option<PathBuf> {
    let path = std::env::current_dir()
        .ok()?
        .join(".edgee")
        .join("credentials.toml");
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

/// Returns the global credentials path (`~/.config/edgee/credentials.toml`).
pub fn global_credentials_path() -> PathBuf {
    #[cfg(windows)]
    {
        let appdata = std::env::var("APPDATA")
            .or_else(|_| std::env::var("USERPROFILE").map(|p| format!("{p}\\AppData\\Roaming")))
            .expect("APPDATA or USERPROFILE not set");
        PathBuf::from(appdata)
            .join("edgee")
            .join("credentials.toml")
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .expect("HOME or USERPROFILE not set");
        PathBuf::from(home).join(".config/edgee/credentials.toml")
    }
}

/// Returns the effective credentials path: local project file if present, global otherwise.
pub fn credentials_path() -> PathBuf {
    local_credentials_path().unwrap_or_else(global_credentials_path)
}

// ---------------------------------------------------------------------------
// Legacy v1 struct (for migration only)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct CredentialsV1 {
    pub api_key: Option<String>,
    pub claude_connection: Option<String>,
    pub org_slug: Option<String>,
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

fn migrate(content: &str) -> Result<(CredentialsFile, bool)> {
    #[derive(Deserialize, Default)]
    struct VersionProbe {
        version: Option<u32>,
    }
    let probe: VersionProbe = toml::from_str(content).unwrap_or_default();

    match probe.version {
        None | Some(1) => {
            // v1: flat file with api_key / claude_connection / org_slug
            let v1: CredentialsV1 = toml::from_str(content).unwrap_or_default();
            let profile = Profile {
                org_slug: v1.org_slug,
                claude: v1
                    .api_key
                    .filter(|k| !k.is_empty())
                    .map(|key| ProviderConfig {
                        api_key: key,
                        connection: v1.claude_connection,
                        ..Default::default()
                    }),
                ..Default::default()
            };
            Ok((wrap_profile(profile), true))
        }
        Some(2) => {
            // v2: Credentials-shaped struct (provider blocks with email/user_id inside)
            let creds: Profile = toml::from_str(content)?;
            Ok((wrap_profile(creds), true))
        }
        Some(3) => {
            // v3: flat Credentials struct with top-level user_token, email, etc.
            let creds: Profile = toml::from_str(content)?;
            Ok((wrap_profile(creds), true))
        }
        Some(v) if v == CURRENT_VERSION => {
            let file: CredentialsFile = toml::from_str(content)?;
            Ok((file, false))
        }
        Some(v) => anyhow::bail!("Unsupported credentials version {v}; please upgrade the CLI"),
    }
}

/// Wrap a single `Profile` into a `CredentialsFile` under the "default" slot.
fn wrap_profile(profile: Profile) -> CredentialsFile {
    let mut profiles = std::collections::BTreeMap::new();
    profiles.insert("default".to_string(), profile);
    CredentialsFile {
        version: Some(CURRENT_VERSION),
        active_profile: Some("default".to_string()),
        profiles,
    }
}

// ---------------------------------------------------------------------------
// Public I/O API
// ---------------------------------------------------------------------------

/// Read the full credentials file.
pub fn read_file() -> Result<CredentialsFile> {
    let path = credentials_path();
    if !path.exists() {
        return Ok(CredentialsFile::default());
    }
    let content = fs::read_to_string(&path)?;
    let (file, migrated) = migrate(&content)?;
    if migrated {
        write_file(&file)?;
    }
    Ok(file)
}

/// Write the full credentials file atomically.
pub fn write_file(file: &CredentialsFile) -> Result<()> {
    let mut file = file.clone();
    file.version = Some(CURRENT_VERSION);
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(&file)?;
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, &content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp_path, &path)?;
    Ok(())
}

/// Read credentials for the active profile.
pub fn read() -> Result<Credentials> {
    let file = read_file()?;
    Ok(file
        .profiles
        .get(&active_profile_name())
        .cloned()
        .unwrap_or_default())
}

/// Write credentials into the active profile slot.
pub fn write(creds: &Credentials) -> Result<()> {
    let mut file = read_file().unwrap_or_default();
    file.profiles.insert(active_profile_name(), creds.clone());
    write_file(&file)
}

// ---------------------------------------------------------------------------
// URL helpers — precedence: env var > active profile > hardcoded default
// ---------------------------------------------------------------------------

pub fn console_base_url() -> String {
    if let Ok(v) = std::env::var("EDGEE_CONSOLE_URL") {
        return v;
    }
    read()
        .ok()
        .and_then(|p| p.console_url)
        .unwrap_or_else(|| "https://www.edgee.ai".to_string())
}

pub fn console_api_base_url() -> String {
    if let Ok(v) = std::env::var("EDGEE_CONSOLE_API_URL") {
        return v;
    }
    read()
        .ok()
        .and_then(|p| p.console_api_url)
        .unwrap_or_else(|| "https://api.edgee.app".to_string())
}

pub fn gateway_base_url() -> String {
    if let Ok(v) = std::env::var("EDGEE_API_URL") {
        return v;
    }
    read()
        .ok()
        .and_then(|p| p.gateway_url)
        .unwrap_or_else(|| "https://api.edgee.ai".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: migrate and unwrap, for concise test assertions.
    fn do_migrate(content: &str) -> (CredentialsFile, bool) {
        migrate(content).expect("migration should succeed")
    }

    #[test]
    fn migrate_v2_to_v4() {
        let v2_config = r#"
version = 2

[claude]
api_key = "sk-edgee-obfuscated-claude-key-xxxxx"
email = "user@example.com"
user_id = "1b1d05f7-0000-0000-0000-000000000000"
connection = "plan"
org_slug = "my-org"

[codex]
api_key = "sk-edgee-obfuscated-codex-key-xxxxx"
email = "user@example.com"
user_id = "1b1d05f7-0000-0000-0000-000000000000"
connection = "plan"
org_slug = "my-org"
"#;

        let (file, migrated) = do_migrate(v2_config);

        assert!(migrated, "v2 config should be marked as migrated");
        assert_eq!(file.version, Some(CURRENT_VERSION));
        assert_eq!(file.active_profile.as_deref(), Some("default"));

        let creds = &file.profiles["default"];

        // v2 has no top-level user_token — it should be None after migration
        assert!(creds.user_token.is_none());

        // Claude provider should be preserved
        let claude = creds.claude.as_ref().expect("claude config should exist");
        assert_eq!(claude.api_key, "sk-edgee-obfuscated-claude-key-xxxxx");
        assert_eq!(claude.connection.as_deref(), Some("plan"));

        // Codex provider should be preserved
        let codex = creds.codex.as_ref().expect("codex config should exist");
        assert_eq!(codex.api_key, "sk-edgee-obfuscated-codex-key-xxxxx");
        assert_eq!(codex.connection.as_deref(), Some("plan"));
    }

    #[test]
    fn migrate_v1_to_v4() {
        let v1_config = r#"
api_key = "sk-edgee-obfuscated-v1-key-xxxxx"
claude_connection = "plan"
org_slug = "my-org"
"#;

        let (file, migrated) = do_migrate(v1_config);

        assert!(migrated, "v1 config should be marked as migrated");
        assert_eq!(file.version, Some(CURRENT_VERSION));

        let creds = &file.profiles["default"];
        assert_eq!(creds.org_slug.as_deref(), Some("my-org"));

        let claude = creds
            .claude
            .as_ref()
            .expect("claude config should exist from v1 api_key");
        assert_eq!(claude.api_key, "sk-edgee-obfuscated-v1-key-xxxxx");
        assert_eq!(claude.connection.as_deref(), Some("plan"));

        assert!(creds.codex.is_none(), "v1 has no codex config");
    }

    #[test]
    fn migrate_v3_to_v4() {
        let v3_config = r#"
version = 3
user_token = "obfuscated-user-token-xxxxx"
email = "user@example.com"
user_id = "1b1d05f7-0000-0000-0000-000000000000"
org_slug = "my-org"
org_id = "9ef51cca-0000-0000-0000-000000000000"

[claude]
api_key = "sk-edgee-obfuscated-claude-key-xxxxx"
api_key_id = "d65711ee-0000-0000-0000-000000000000"
connection = "plan"
"#;

        let (file, migrated) = do_migrate(v3_config);

        assert!(migrated, "v3 config should be marked as migrated");
        assert_eq!(file.version, Some(CURRENT_VERSION));
        assert_eq!(file.active_profile.as_deref(), Some("default"));

        let creds = &file.profiles["default"];
        assert_eq!(
            creds.user_token.as_deref(),
            Some("obfuscated-user-token-xxxxx")
        );
        assert_eq!(creds.email.as_deref(), Some("user@example.com"));
        assert_eq!(creds.org_slug.as_deref(), Some("my-org"));

        let claude = creds.claude.as_ref().expect("claude should exist");
        assert_eq!(claude.api_key, "sk-edgee-obfuscated-claude-key-xxxxx");
    }

    #[test]
    fn parse_v4_current() {
        let v4_config = r#"
version = 4
active_profile = "default"

[profiles.default]
user_token = "obfuscated-user-token-xxxxx"
email = "user@example.com"
user_id = "1b1d05f7-0000-0000-0000-000000000000"
org_slug = "my-org"
org_id = "9ef51cca-0000-0000-0000-000000000000"

[profiles.default.claude]
api_key = "sk-edgee-obfuscated-claude-key-xxxxx"
api_key_id = "d65711ee-0000-0000-0000-000000000000"
connection = "plan"

[profiles.default.codex]
api_key = "sk-edgee-obfuscated-codex-key-xxxxx"
api_key_id = "a1b2c3d4-0000-0000-0000-000000000000"
connection = "plan"

[profiles.work]
user_token = "work-token-xxxxx"
email = "user@work.com"
org_slug = "work-org"
org_id = "aaaabbbb-0000-0000-0000-000000000000"
"#;

        let (file, migrated) = do_migrate(v4_config);

        assert!(!migrated, "v4 config should not need migration");
        assert_eq!(file.version, Some(4));
        assert_eq!(file.active_profile.as_deref(), Some("default"));
        assert_eq!(file.profiles.len(), 2);

        let default = &file.profiles["default"];
        assert_eq!(
            default.user_token.as_deref(),
            Some("obfuscated-user-token-xxxxx")
        );
        assert_eq!(default.org_slug.as_deref(), Some("my-org"));

        let claude = default.claude.as_ref().expect("claude should exist");
        assert_eq!(claude.api_key, "sk-edgee-obfuscated-claude-key-xxxxx");

        let codex = default.codex.as_ref().expect("codex should exist");
        assert_eq!(codex.api_key, "sk-edgee-obfuscated-codex-key-xxxxx");

        let work = &file.profiles["work"];
        assert_eq!(work.email.as_deref(), Some("user@work.com"));
        assert_eq!(work.org_slug.as_deref(), Some("work-org"));
    }

    #[test]
    fn migrate_v2_ignores_unknown_provider_fields() {
        let v2_config = r#"
version = 2

[claude]
api_key = "sk-edgee-obfuscated-key-xxxxx"
email = "user@example.com"
user_id = "1b1d05f7-0000-0000-0000-000000000000"
connection = "plan"
org_slug = "my-org"
"#;

        let (file, migrated) = do_migrate(v2_config);

        assert!(migrated);
        let creds = &file.profiles["default"];
        let claude = creds.claude.as_ref().expect("claude should exist");
        assert_eq!(claude.api_key, "sk-edgee-obfuscated-key-xxxxx");
        assert_eq!(claude.connection.as_deref(), Some("plan"));
        // api_key_id didn't exist in v2, should be None
        assert!(claude.api_key_id.is_none());
    }

    #[test]
    fn migrate_unsupported_version_errors() {
        let future_config = "version = 99\n";
        let result = migrate(future_config);
        assert!(result.is_err());
    }

    #[test]
    fn migrate_empty_config() {
        let (file, migrated) = do_migrate("");
        assert!(migrated);
        let creds = &file.profiles["default"];
        assert!(creds.claude.is_none());
        assert!(creds.codex.is_none());
    }
}
