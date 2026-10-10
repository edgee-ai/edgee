//! Codex model catalog wiring, shared by `launch codex` and `launch codex-desktop`.
//!
//! Codex builds its model picker and per-model metadata (instructions, reasoning
//! levels, context window) from a catalog. Without one it falls back to the OpenAI
//! models bundled in the binary, so models routed through Edgee (Claude on Bedrock,
//! Kimi, GLM…) never reach the picker and run on fallback metadata. The gateway serves
//! a Codex-shaped catalog on `/v1/models?catalog=edgee&client_version=…`: the org's
//! Edgee models, behind OpenAI's own entries when the caller forwards a ChatGPT
//! credential.
//!
//! How Codex gets it depends on its sign-in, read from `codex login status`:
//!
//! - **Signed in** (ChatGPT or an OpenAI API key): `model_catalog_url` on our provider.
//!   Codex fetches it itself with its own credential and refreshes it every few
//!   minutes. `features.api_key_model_discovery` is required for API-key sign-ins on
//!   Codex builds where it isn't on by default; ChatGPT sign-ins ignore it.
//! - **Signed out**: Codex only fetches a catalog when it holds a credential. Handing it
//!   our Edgee key as a provider bearer would shadow the ChatGPT token of a user who
//!   signs in mid-session, so instead we fetch the catalog here and pass it as
//!   `model_catalog_json`, a file Codex loads as is.
//! - **Unknown** (status didn't run or said something else): nothing, as before.
//!
//! An explicit catalog replaces Codex's bundled one, and a body Codex can't parse
//! leaves the picker empty. So every path first fetches the catalog and only wires it
//! when it comes back in Codex's shape. A gateway that predates the endpoint answers
//! with the OpenAI listing and the launch carries on exactly as before.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::commands::util::plugins::config::toml_string;

/// Query that selects the Edgee catalog on the gateway's `/v1/models`.
const CATALOG_QUERY: &str = "catalog=edgee";

/// Placeholder sent as `client_version` when Codex's own version can't be read. The
/// gateway only needs the parameter to be present.
const UNKNOWN_CLIENT_VERSION: &str = "0.0.0";

/// Codex's sign-in, as `codex login status` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    SignedIn,
    SignedOut,
    Unknown,
}

/// `model_catalog_url` for a gateway `/v1` base URL.
pub fn catalog_url(base_url: &str) -> String {
    format!("{base_url}/models?{CATALOG_QUERY}")
}

/// `-c` overrides wiring the catalog into a `codex` CLI launch through `provider_id`.
/// Empty when the catalog is unavailable or the sign-in can't be read.
pub async fn cli_args(codex: &OsStr, provider_id: &str, base_url: &str, api_key: &str) -> Vec<String> {
    let sign_in = sign_in(codex).await;
    if sign_in == SignIn::Unknown {
        return Vec::new();
    }
    let version = codex_version(codex).await;
    let Some(body) = fetch(base_url, api_key, version.as_deref()).await else {
        return Vec::new();
    };
    match sign_in {
        SignIn::SignedIn => vec![
            format!(
                "model_providers.{provider_id}.model_catalog_url={}",
                toml_string(&catalog_url(base_url))
            ),
            "features.api_key_model_discovery=true".to_string(),
        ],
        SignIn::SignedOut => match write_catalog_file(&body) {
            Ok(path) => vec![format!(
                "model_catalog_json={}",
                toml_string(&path.to_string_lossy())
            )],
            Err(_) => Vec::new(),
        },
        SignIn::Unknown => Vec::new(),
    }
}

/// Whether the gateway serves the Codex catalog for this key: the precondition for
/// putting `model_catalog_url` in a config the ChatGPT desktop app reads.
pub async fn is_served(base_url: &str, api_key: &str) -> bool {
    fetch(base_url, api_key, None).await.is_some()
}

async fn sign_in(codex: &OsStr) -> SignIn {
    let output = tokio::process::Command::new(codex)
        .args(["login", "status"])
        .stdin(std::process::Stdio::null())
        .output()
        .await;
    match output {
        Ok(out) => classify_sign_in(
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(_) => SignIn::Unknown,
    }
}

/// `codex login status` exits 0 when signed in, and prints `Not logged in` (on
/// stderr, as of 0.162) when not. Anything else is not trusted either way.
fn classify_sign_in(success: bool, stdout: &str, stderr: &str) -> SignIn {
    if success {
        SignIn::SignedIn
    } else if stdout.contains("Not logged in") || stderr.contains("Not logged in") {
        SignIn::SignedOut
    } else {
        SignIn::Unknown
    }
}

async fn codex_version(codex: &OsStr) -> Option<String> {
    let out = tokio::process::Command::new(codex)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

/// `codex-cli 0.162.0` → `0.162.0`.
fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .last()
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

/// The catalog body, only when it is in Codex's shape with at least one model.
async fn fetch(base_url: &str, api_key: &str, client_version: Option<&str>) -> Option<Vec<u8>> {
    let url = format!(
        "{}&client_version={}",
        catalog_url(base_url),
        client_version.unwrap_or(UNKNOWN_CLIENT_VERSION)
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client
        .get(&url)
        .header("x-api-key", api_key)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.bytes().await.ok()?.to_vec();
    is_codex_catalog(&body).then_some(body)
}

/// A top-level, non-empty `models` array. A gateway without the Codex catalog answers
/// with the OpenAI listing (`data`), which Codex can't load.
fn is_codex_catalog(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("models")?.as_array().map(|m| !m.is_empty()))
        .unwrap_or(false)
}

fn write_catalog_file(body: &[u8]) -> Result<PathBuf> {
    let path = crate::config::global_data_dir()
        .join("codex")
        .join("model_catalog.json");
    write_atomically(&path, body)?;
    Ok(path)
}

/// Write via a sibling temp file and a rename, so a concurrent launch never hands
/// Codex a half-written catalog.
fn write_atomically(path: &Path, body: &[u8]) -> Result<()> {
    let dir = path.parent().context("catalog path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(format!(".model_catalog.{}.tmp", std::process::id()));
    std::fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_is_read_from_exit_code_and_message() {
        assert_eq!(
            classify_sign_in(true, "", "Logged in using ChatGPT\n"),
            SignIn::SignedIn
        );
        assert_eq!(
            classify_sign_in(false, "", "Not logged in\n"),
            SignIn::SignedOut
        );
        assert_eq!(
            classify_sign_in(false, "Not logged in\n", ""),
            SignIn::SignedOut
        );
        // An error we don't recognize must not be read as signed out.
        assert_eq!(
            classify_sign_in(false, "", "error: unexpected argument 'status'\n"),
            SignIn::Unknown
        );
    }

    #[test]
    fn version_is_the_last_token_when_numeric() {
        assert_eq!(parse_version("codex-cli 0.162.0\n").as_deref(), Some("0.162.0"));
        assert_eq!(parse_version("0.1.0").as_deref(), Some("0.1.0"));
        assert_eq!(parse_version("codex-cli"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn only_a_non_empty_models_array_is_a_codex_catalog() {
        assert!(is_codex_catalog(br#"{"models":[{"slug":"a/b"}]}"#));
        assert!(!is_codex_catalog(br#"{"models":[]}"#));
        assert!(!is_codex_catalog(br#"{"object":"list","data":[{"id":"a/b"}]}"#));
        assert!(!is_codex_catalog(b"not json"));
    }

    #[test]
    fn catalog_url_selects_the_edgee_catalog() {
        assert_eq!(
            catalog_url("https://api.edgee.ai/v1"),
            "https://api.edgee.ai/v1/models?catalog=edgee"
        );
    }

    #[test]
    fn catalog_file_is_replaced_whole() {
        let dir = std::env::temp_dir().join(format!("edgee-cat-{}", uuid::Uuid::new_v4()));
        let path = dir.join("codex").join("model_catalog.json");
        write_atomically(&path, b"{\"models\":[1]}").unwrap();
        write_atomically(&path, b"{\"models\":[2]}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"models\":[2]}");
        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temp file left behind");
        std::fs::remove_dir_all(&dir).ok();
    }
}
