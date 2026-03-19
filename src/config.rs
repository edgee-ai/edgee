use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Credentials {
    pub api_key: String,
    pub claude_connection: Option<String>, // "plan" | "api"
    pub org_slug: Option<String>,
}

pub fn credentials_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .expect("HOME or USERPROFILE not set");
    PathBuf::from(home).join(".config/edgee/credentials.toml")
}

pub fn read() -> Result<Credentials> {
    let path = credentials_path();
    if !path.exists() {
        return Ok(Credentials::default());
    }
    let content = fs::read_to_string(&path)?;
    let creds: Credentials = toml::from_str(&content)?;
    Ok(creds)
}

pub fn write(creds: &Credentials) -> Result<()> {
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(creds)?;
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

pub fn api_base_url() -> String {
    std::env::var("EDGEE_API_URL").unwrap_or_else(|_| "https://api.edgee.ai".to_string())
}
