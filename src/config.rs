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
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .expect("HOME or USERPROFILE not set");
    PathBuf::from(home).join(".config/edgee/credentials.toml")
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
                claude: v1.api_key.filter(|k| !k.is_empty()).map(|key| ProviderConfig {
                    api_key: key,
                    connection: v1.claude_connection,
                    ..Default::default()
                }),
                ..Default::default()
            };
            Ok((creds, true))
        }
        Some(2) => {
            let creds: Credentials = toml::from_str(content)?;
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
