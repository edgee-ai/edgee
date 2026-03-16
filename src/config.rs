use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Credentials {
    pub api_key: String,
    pub claude_connection: Option<String>, // "plan" | "api"
}

pub fn credentials_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
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
    fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp_path, &path)?;
    Ok(())
}
