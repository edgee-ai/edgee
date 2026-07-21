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
    /// The gateway base URL configured for this org in the console (region or
    /// self-hosted). Absent/empty when never set; the launch path then falls
    /// back to a local override or the built-in default.
    #[serde(default, rename = "gateway_api_url")]
    pub gateway_url: Option<String>,
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
    /// Always present in the server response. A key with no expiry is sent as
    /// the Go zero-value sentinel (`0001-01-01T00:00:00Z`), not omitted.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: time::OffsetDateTime,
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

/// A BYOK provider key (`GET /v1/organizations/{org}/provider-keys`). Only the
/// fields needed to mark catalog models available via the user's own keys are read.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderKey {
    pub provider: String,
    #[serde(default)]
    pub active: bool,
}

/// Subset of `GET /v1/organizations/{org}/billing` used to decide whether the org
/// has paid access to AI Gateway routing (fallback/reroute).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OrgBilling {
    #[serde(default)]
    pub ai_gateway_plan: Option<String>,
    #[serde(default)]
    pub ai_gateway_subscription_status: Option<String>,
}

impl OrgBilling {
    /// A non-free plan, or an active trial, grants routing access.
    pub fn is_paying(&self) -> bool {
        let paid_plan = matches!(
            self.ai_gateway_plan.as_deref(),
            Some("team") | Some("enterprise") | Some("custom")
        );
        let trialing = self.ai_gateway_subscription_status.as_deref() == Some("trial");
        paid_plan || trialing
    }
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
    /// Whether the model is covered by the user's plan for fallback/reroute.
    /// Plan-covered models are offered first in the settings pickers.
    #[serde(default)]
    pub plan_fallback: bool,
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

/// Self-serve spend-limit status for the logged-in user on an org
/// (`GET /v1/organizations/{org}/usage-limit-status`). Always resolves to the
/// caller, unlike the admin-only per-member settings endpoint. Used to warn
/// the user before they hit their cap.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageLimitStatus {
    pub has_limit: bool,
    #[serde(default)]
    pub max_usage: Option<f64>,
    #[serde(default)]
    pub used_credits: Option<u64>,
    #[serde(default)]
    pub period: Option<String>,
    #[serde(default)]
    pub percent_used: Option<f64>,
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

    /// Fetches a single organization (`GET /v1/organizations/{org}`). Used at
    /// launch to read the org's configured `gateway_api_url` fresh, so a console
    /// change takes effect on the next launch without re-login.
    pub async fn get_organization(&self, org_id: &str) -> Result<Organization> {
        let url = format!("{}/v1/organizations/{}", self.base_url, org_id);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to fetch organization")?;
        check_status(&resp, "fetch organization")?;
        resp.json().await.context("Invalid organization response")
    }

    /// Lists the gateway model catalog (with `plan_fallback`, `aliases`, etc.) used
    /// to offer fallback/reroute targets. Served by the console API
    /// (`console_api_base_url`, e.g. `api.edgee.app`) — not the gateway, whose
    /// `/v1/models` is the stripped OpenAI listing.
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

    /// Lists the org's BYOK provider keys. Used to flag catalog models reachable
    /// through the user's own keys. Returns a raw array (no `{ data: [...] }` wrapper).
    pub async fn list_provider_keys(&self, org_id: &str) -> Result<Vec<ProviderKey>> {
        let url = format!(
            "{}/v1/organizations/{}/provider-keys",
            self.base_url, org_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to list provider keys")?;
        check_status(&resp, "list provider keys")?;
        resp.json().await.context("Invalid provider keys response")
    }

    /// Whether the org has a paid AI Gateway plan (or active trial), which is what
    /// unlocks fallback/reroute. Mirrors the console's `useAIGatewayPaying`: a
    /// non-free `ai_gateway_plan` or a `trial` subscription status counts as paying.
    pub async fn org_is_paying(&self, org_id: &str) -> Result<bool> {
        let url = format!("{}/v1/organizations/{}/billing", self.base_url, org_id);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to fetch billing")?;
        check_status(&resp, "fetch billing")?;
        let billing: OrgBilling = resp.json().await.context("Invalid billing response")?;
        Ok(billing.is_paying())
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

    /// Fetches a single coding-agent key by id.
    ///
    /// `Ok(None)` means the key no longer exists (HTTP 404) — e.g. it was deleted
    /// in the console — so the caller can re-provision it. `Err` is reserved for
    /// transient/other failures (network, auth, 5xx) where the key's existence is
    /// unknown and the caller must not assume it's gone.
    pub async fn get_key_by_id(&self, org_id: &str, key_id: &str) -> Result<Option<ApiKeyItem>> {
        let url = format!(
            "{}/v1/organizations/{}/api_keys/{}",
            self.base_url, org_id, key_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to fetch API key")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        check_status(&resp, "fetch API key")?;
        resp.json()
            .await
            .map(Some)
            .context("Invalid API key response")
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

    /// Fetches the caller's own spend-limit status on an org
    /// (`GET /v1/organizations/{org}/usage-limit-status`). Self-serve — any
    /// org role — so this always reports the logged-in user, not an
    /// arbitrary member.
    pub async fn get_usage_limit_status(&self, org_id: &str) -> Result<UsageLimitStatus> {
        let url = format!(
            "{}/v1/organizations/{}/usage-limit-status",
            self.base_url, org_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Failed to fetch usage limit status")?;
        check_status(&resp, "fetch usage limit status")?;
        resp.json()
            .await
            .context("Invalid usage limit status response")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn billing(plan: Option<&str>, status: Option<&str>) -> OrgBilling {
        OrgBilling {
            ai_gateway_plan: plan.map(str::to_string),
            ai_gateway_subscription_status: status.map(str::to_string),
        }
    }

    #[test]
    fn is_paying_for_non_free_plans_and_trial() {
        assert!(billing(Some("team"), None).is_paying());
        assert!(billing(Some("enterprise"), None).is_paying());
        assert!(billing(Some("custom"), None).is_paying());
        // Trial counts even with a free/absent plan.
        assert!(billing(Some("free"), Some("trial")).is_paying());
        assert!(billing(None, Some("trial")).is_paying());
    }

    #[test]
    fn not_paying_for_free_or_absent_plan() {
        assert!(!billing(Some("free"), Some("active")).is_paying());
        assert!(!billing(None, None).is_paying());
        assert!(!billing(None, Some("cancelled")).is_paying());
    }

    #[test]
    fn deserializes_expires_at_sentinel_and_real_timestamp() {
        let no_expiry: ApiKeyItem =
            serde_json::from_str(r#"{"id":"k1","expires_at":"0001-01-01T00:00:00Z"}"#).unwrap();
        assert_eq!(no_expiry.expires_at.year(), 1);

        let with_expiry: ApiKeyItem =
            serde_json::from_str(r#"{"id":"k2","expires_at":"2030-06-15T14:30:00Z"}"#).unwrap();
        assert_eq!(with_expiry.expires_at.year(), 2030);
    }
}
