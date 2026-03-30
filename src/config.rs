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

const CURRENT_VERSION: u32 = 3;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Credentials {
    pub version: Option<u32>,
    pub user_token: Option<String>,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub org_slug: Option<String>,
    pub org_id: Option<String>,
    pub claude: Option<ProviderConfig>,
    pub codex: Option<ProviderConfig>,
}

pub fn credentials_path() -> PathBuf {
    #[cfg(windows)]
    {
        let appdata = std::env::var("APPDATA")
            .or_else(|_| {
                std::env::var("USERPROFILE")
                    .map(|p| format!("{p}\\AppData\\Roaming"))
            })
            .expect("APPDATA or USERPROFILE not set");
        PathBuf::from(appdata).join("edgee").join("credentials.toml")
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .expect("HOME or USERPROFILE not set");
        PathBuf::from(home).join(".config/edgee/credentials.toml")
    }
}

#[derive(Debug, Deserialize, Default)]
struct CredentialsV1 {
    pub api_key: Option<String>,
    pub claude_connection: Option<String>,
    pub org_slug: Option<String>,
}

fn migrate(content: &str) -> Result<(Credentials, bool)> {
    #[derive(Deserialize, Default)]
    struct VersionProbe {
        version: Option<u32>,
    }
    let probe: VersionProbe = toml::from_str(content).unwrap_or_default();

    match probe.version {
        None | Some(1) => {
            let v1: CredentialsV1 = toml::from_str(content).unwrap_or_default();
            let creds = Credentials {
                version: Some(CURRENT_VERSION),
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
            Ok((creds, true))
        }
        Some(2) => {
            let mut creds: Credentials = toml::from_str(content)?;
            creds.version = Some(CURRENT_VERSION);
            Ok((creds, true))
        }
        Some(v) if v == CURRENT_VERSION => {
            let creds: Credentials = toml::from_str(content)?;
            Ok((creds, false))
        }
        Some(v) => anyhow::bail!("Unsupported credentials version {v}; please upgrade the CLI"),
    }
}

pub fn read() -> Result<Credentials> {
    let path = credentials_path();
    if !path.exists() {
        return Ok(Credentials::default());
    }
    let content = fs::read_to_string(&path)?;
    let (creds, migrated) = migrate(&content)?;
    if migrated {
        write(&creds)?;
    }
    Ok(creds)
}

pub fn write(creds: &Credentials) -> Result<()> {
    let mut creds = creds.clone();
    creds.version = Some(CURRENT_VERSION);
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(&creds)?;
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

pub fn console_base_url() -> String {
    std::env::var("EDGEE_CONSOLE_URL").unwrap_or_else(|_| "https://www.edgee.ai".to_string())
}

pub fn console_api_base_url() -> String {
    std::env::var("EDGEE_CONSOLE_API_URL").unwrap_or_else(|_| "https://api.edgee.app".to_string())
}

pub fn gateway_base_url() -> String {
    std::env::var("EDGEE_API_URL").unwrap_or_else(|_| "https://api.edgee.ai".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_v2_to_v3() {
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

        let (creds, migrated) = migrate(v2_config).expect("v2 migration should succeed");

        assert!(migrated, "v2 config should be marked as migrated");
        assert_eq!(creds.version, Some(CURRENT_VERSION));

        // v2 has no top-level user_token — it should be None after migration
        assert!(creds.user_token.is_none());

        // Claude provider should be preserved
        let claude = creds.claude.expect("claude config should exist");
        assert_eq!(claude.api_key, "sk-edgee-obfuscated-claude-key-xxxxx");
        assert_eq!(claude.connection.as_deref(), Some("plan"));

        // Codex provider should be preserved
        let codex = creds.codex.expect("codex config should exist");
        assert_eq!(codex.api_key, "sk-edgee-obfuscated-codex-key-xxxxx");
        assert_eq!(codex.connection.as_deref(), Some("plan"));
    }

    #[test]
    fn migrate_v1_to_v3() {
        let v1_config = r#"
api_key = "sk-edgee-obfuscated-v1-key-xxxxx"
claude_connection = "plan"
org_slug = "my-org"
"#;

        let (creds, migrated) = migrate(v1_config).expect("v1 migration should succeed");

        assert!(migrated, "v1 config should be marked as migrated");
        assert_eq!(creds.version, Some(CURRENT_VERSION));
        assert_eq!(creds.org_slug.as_deref(), Some("my-org"));

        let claude = creds
            .claude
            .expect("claude config should exist from v1 api_key");
        assert_eq!(claude.api_key, "sk-edgee-obfuscated-v1-key-xxxxx");
        assert_eq!(claude.connection.as_deref(), Some("plan"));

        assert!(creds.codex.is_none(), "v1 has no codex config");
    }

    #[test]
    fn parse_v3_current() {
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

[codex]
api_key = "sk-edgee-obfuscated-codex-key-xxxxx"
api_key_id = "a1b2c3d4-0000-0000-0000-000000000000"
connection = "plan"
"#;

        let (creds, migrated) = migrate(v3_config).expect("v3 parse should succeed");

        assert!(!migrated, "v3 config should not need migration");
        assert_eq!(creds.version, Some(3));
        assert_eq!(
            creds.user_token.as_deref(),
            Some("obfuscated-user-token-xxxxx")
        );
        assert_eq!(creds.email.as_deref(), Some("user@example.com"));
        assert_eq!(creds.org_slug.as_deref(), Some("my-org"));
        assert_eq!(
            creds.org_id.as_deref(),
            Some("9ef51cca-0000-0000-0000-000000000000")
        );

        let claude = creds.claude.expect("claude should exist");
        assert_eq!(claude.api_key, "sk-edgee-obfuscated-claude-key-xxxxx");
        assert_eq!(
            claude.api_key_id.as_deref(),
            Some("d65711ee-0000-0000-0000-000000000000")
        );

        let codex = creds.codex.expect("codex should exist");
        assert_eq!(codex.api_key, "sk-edgee-obfuscated-codex-key-xxxxx");
    }

    #[test]
    fn migrate_v2_ignores_unknown_provider_fields() {
        // v2 configs have email/user_id/org_slug inside provider sections
        // which don't exist in the new ProviderConfig — they should be silently ignored
        let v2_config = r#"
version = 2

[claude]
api_key = "sk-edgee-obfuscated-key-xxxxx"
email = "user@example.com"
user_id = "1b1d05f7-0000-0000-0000-000000000000"
connection = "plan"
org_slug = "my-org"
"#;

        let (creds, migrated) = migrate(v2_config).expect("v2 with extra fields should parse");

        assert!(migrated);
        let claude = creds.claude.expect("claude should exist");
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
        let (creds, migrated) = migrate("").expect("empty config should parse as v1");
        assert!(migrated);
        assert!(creds.claude.is_none());
        assert!(creds.codex.is_none());
    }
}
