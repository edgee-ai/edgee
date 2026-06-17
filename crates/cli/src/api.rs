use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

pub struct ApiClient {
    http: reqwest::Client,
    base_url: String,
}

#[derive(Deserialize)]
pub struct Organization {
    pub id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Deserialize)]
struct ListResponse<T> {
    data: Vec<T>,
}

/// Console API error body: `{ "error": { "message": "...", ... } }`.
#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

#[derive(Deserialize)]
pub struct ApiKeyItem {
    pub id: String,
    pub key: Option<String>,
    /// True only when the get-or-create endpoint minted a new key (omitted/false
    /// when an existing key was returned). Gates first-run onboarding.
    #[serde(default)]
    pub created: bool,
    /// Current compression config on the key (absent when never configured).
    #[serde(default)]
    pub compression: Option<Compression>,
    /// Models the key fails over to (empty when none configured). The on/off
    /// state is derived from whether this list is non-empty.
    #[serde(default)]
    pub fallbacks: Vec<ModelRoute>,
    /// Models the key reroutes requests to (empty when no reroute is configured).
    #[serde(default)]
    pub reroutes: Vec<ModelRoute>,
}

/// Compression techniques to apply to a coding-agent key. Each flag maps to a
/// composable technique on the gateway; the wizard sets all three explicitly so
/// the user's choice fully determines the key configuration.
///
/// The server models these as nullable bools ("inherit from org/group scope"
/// when null); on read we treat null/missing as `false`, and on write we always
/// send explicit values so the user's choice is authoritative.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Compression {
    #[serde(default, deserialize_with = "de_bool_lenient")]
    pub tool_result_trimming: bool,
    #[serde(default, deserialize_with = "de_bool_lenient")]
    pub tool_surface_reduction: bool,
    #[serde(default, deserialize_with = "de_bool_lenient")]
    pub output_brevity: bool,
}

/// A single model-routing entry (used by both `reroutes` and `fallbacks`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    pub model: String,
}

/// A model in the gateway catalog (`GET /v1/models`). Only the fields needed to
/// derive a selectable routing identifier are deserialized.
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayModel {
    pub model_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Provider name → provider config (config is ignored; only the keys matter).
    #[serde(default)]
    pub providers: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub active: bool,
}

impl GatewayModel {
    /// A valid routing identifier the server's `IsValidModel` accepts: prefer the
    /// first alias, otherwise `<provider>/<model_id>` (providers sorted for
    /// determinism). Returns `None` for a model with neither.
    pub fn route_identifier(&self) -> Option<String> {
        if let Some(alias) = self.aliases.first() {
            return Some(alias.clone());
        }
        let mut providers: Vec<&String> = self.providers.keys().collect();
        providers.sort();
        providers
            .first()
            .map(|p| format!("{}/{}", p, self.model_id))
    }
}

/// Full mutable settings sent to the key-update endpoint. Serializes to
/// `{ "compression": {...}, "fallback": bool, "fallbacks": [...] | null,
/// "reroutes": [...] | null }`. A `None` list clears that route (the server
/// distinguishes a present-null field from an omitted one). `fallback` is the
/// on/off switch; `fallbacks` are the models to fail over to.
#[derive(Debug, Clone, Serialize)]
pub struct KeySettings {
    pub compression: Compression,
    pub fallback: bool,
    pub fallbacks: Option<Vec<ModelRoute>>,
    pub reroutes: Option<Vec<ModelRoute>>,
}

/// Deserializes a nullable/absent bool field as `false` rather than erroring.
fn de_bool_lenient<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(false))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub total_requests: u64,
    pub total_cost: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cached_input_tokens: u64,
    pub total_cache_creation_input_tokens: u64,
    pub total_reasoning_output_tokens: u64,
    pub total_token_cost_savings: u64,
    pub total_errors: u64,
    pub total_uncompressed_tools_tokens: u64,
    pub total_compressed_tools_tokens: u64,
    pub tool_compression_stats: Option<HashMap<String, ToolCompressionStat>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCompressionStat {
    pub count: u64,
    pub before: u64,
    pub after: u64,
}

impl ApiClient {
    pub fn new(token: &str) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .context("Invalid token")?,
        );

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            http,
            base_url: crate::config::console_api_base_url(),
        })
    }

    pub async fn list_organizations(&self) -> Result<Vec<Organization>> {
        let url = format!("{}/v1/organizations", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to list organizations")?;
        check_status(&resp, "list organizations")?;
        let body: ListResponse<Organization> =
            resp.json().await.context("Invalid organization response")?;
        Ok(body.data)
    }

    /// Lists the gateway model catalog. Used to offer fallback/reroute targets.
    pub async fn list_models(&self) -> Result<Vec<GatewayModel>> {
        let url = format!("{}/v1/models", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to list models")?;
        check_status(&resp, "list models")?;
        resp.json().await.context("Invalid models response")
    }

    pub async fn get_or_create_key(
        &self,
        org_id: &str,
        coding_assistant: &str,
    ) -> Result<ApiKeyItem> {
        let url = format!(
            "{}/v1/organizations/{}/api_keys/get-or-create",
            self.base_url, org_id
        );
        let body = serde_json::json!({ "coding_assistant": coding_assistant, "compression": true });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to get or create API key")?;
        check_status(&resp, "get or create API key")?;
        resp.json().await.context("Invalid API key response")
    }

    /// Applies the chosen compression techniques to an existing coding-agent key.
    pub async fn update_key_compression(
        &self,
        org_id: &str,
        key_id: &str,
        compression: &Compression,
    ) -> Result<()> {
        let url = format!(
            "{}/v1/organizations/{}/api_keys/{}",
            self.base_url, org_id, key_id
        );
        let body = serde_json::json!({ "compression": compression });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to update key compression")?;
        check_status(&resp, "update key compression")?;
        Ok(())
    }

    /// Applies the full settings bundle (compression + fallback + reroutes) to an
    /// existing coding-agent key. Surfaces the server's error message directly
    /// (e.g. the paid-seat requirement for fallback/reroute).
    pub async fn update_key_settings(
        &self,
        org_id: &str,
        key_id: &str,
        settings: &KeySettings,
    ) -> Result<()> {
        let url = format!(
            "{}/v1/organizations/{}/api_keys/{}",
            self.base_url, org_id, key_id
        );
        let resp = self
            .http
            .post(&url)
            .json(settings)
            .send()
            .await
            .context("Failed to update key settings")?;

        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        let server_msg = serde_json::from_str::<ErrorEnvelope>(&body)
            .ok()
            .and_then(|e| e.error)
            .and_then(|e| e.message)
            .filter(|m| !m.is_empty());
        match (status.as_u16(), server_msg) {
            (401, _) => {
                anyhow::bail!("Authentication expired. Please run `edgee auth login` again.")
            }
            (_, Some(msg)) => anyhow::bail!("{msg}"),
            _ => anyhow::bail!("Failed to update key settings: HTTP {status}"),
        }
    }

    pub async fn set_session_cli_version(
        &self,
        org_id: &str,
        session_id: &str,
        version: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/v1/organizations/{}/sessions/{}/cli-version",
            self.base_url, org_id, session_id
        );
        let body = serde_json::json!({ "version": version });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to report CLI version")?;
        check_status(&resp, "report CLI version")?;
        Ok(())
    }

    pub async fn get_session_stats(&self, org_id: &str, session_id: &str) -> Result<SessionStats> {
        let url = format!(
            "{}/v1/organizations/{}/sessions/{}/end",
            self.base_url, org_id, session_id
        );
        let resp = self
            .http
            .post(&url)
            .send()
            .await
            .context("Failed to get session stats")?;
        check_status(&resp, "get session stats")?;
        resp.json().await.context("Invalid session stats response")
    }
}

fn check_status(resp: &reqwest::Response, action: &str) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    match status.as_u16() {
        401 => anyhow::bail!("Authentication expired. Please run `edgee auth login` again."),
        403 => anyhow::bail!(
            "Permission denied: you don't have access to {action} on this organization."
        ),
        404 => {
            anyhow::bail!("Not found while trying to {action}. The resource may have been deleted.")
        }
        _ => anyhow::bail!("Failed to {action}: HTTP {status}"),
    }
}
