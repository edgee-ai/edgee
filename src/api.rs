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
    /// Org-level kill switch for Edgee's MCP auto-injection, set in the console.
    /// When true the launch path must not pass `--mcp-config` or the Edgee
    /// system prompt to the coding agent, and must not prompt for the local
    /// preference. Absent (older API) deserializes to `false` — injection stays
    /// allowed.
    #[serde(default)]
    pub mcp_injection_disabled: bool,
}

#[derive(Deserialize)]
struct ListResponse<T> {
    data: Vec<T>,
}

/// Console API error body: `{ "error": { "message": "...", "params": [...] } }`.
#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
    #[serde(default)]
    params: Vec<ErrorParam>,
}

#[derive(Deserialize)]
struct ErrorParam {
    message: Option<String>,
}

impl ErrorEnvelope {
    /// Best human-readable message: the specific per-parameter messages when
    /// present (e.g. "Must be one of …"), otherwise the top-level message.
    fn describe(&self) -> Option<String> {
        let error = self.error.as_ref()?;
        let params: Vec<String> = error
            .params
            .iter()
            .filter_map(|p| p.message.clone())
            .collect();
        if !params.is_empty() {
            return Some(params.join("; "));
        }
        error.message.clone().filter(|m| !m.is_empty())
    }
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
    /// Effective byok_only (org > squad > key resolved server-side): when true,
    /// the gateway only routes this key through the org's/user's own BYOK
    /// provider keys and rejects requests for models with no matching
    /// credentials. Absent/false means no such restriction.
    #[serde(default)]
    pub byok_only: bool,
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

/// One upstream provider's configuration for a catalog model.
///
/// The rate fields are US dollars per million tokens. The server omits a rate
/// when it is zero, so a missing field means free, not unknown.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GatewayModelProvider {
    /// Maximum context window, in tokens, when the model is served by this
    /// provider. Providers disagree (e.g. Anthropic's 1M vs Cursor's 256k for
    /// the same model), so `GatewayModel::preferred_provider` picks between them.
    #[serde(default)]
    pub context_max_size: u64,
    #[serde(default)]
    pub input_token_cost_per_million: f64,
    #[serde(default)]
    pub output_token_cost_per_million: f64,
    #[serde(default)]
    pub cached_input_token_cost_per_million: f64,
    #[serde(default)]
    pub cache_creation_input_token_cost_per_million: f64,
}

/// Per-million-token rates for a model, in US dollars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GatewayModelCost {
    pub input: f64,
    pub output: f64,
    /// Reading an existing prompt-cache entry.
    pub cache_read: f64,
    /// Writing a new prompt-cache entry.
    pub cache_write: f64,
}

/// Catalog providers that are coding-app subscriptions rather than LLM APIs.
/// Reaching them means going through Cursor or GitHub Copilot itself, so they are
/// not valid fallback/reroute upstreams for another agent's traffic.
pub const APP_PROVIDERS: [&str; 2] = ["cursor", "github_copilot"];

/// True for a catalog provider that is a coding-app subscription — see
/// [`APP_PROVIDERS`].
pub fn is_app_provider(provider: &str) -> bool {
    APP_PROVIDERS.contains(&provider)
}

/// A model in the gateway catalog (`GET /v1/models`). Only the fields needed to
/// derive a selectable routing identifier and its context window are
/// deserialized.
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayModel {
    pub model_id: String,
    /// Model author (`anthropic`, `openai`, …). Combined with `model_id` this is
    /// the id the gateway's own `/v1/models` listing exposes; see `catalog_id`.
    #[serde(default)]
    pub author_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Provider name → that provider's config for this model.
    #[serde(default)]
    pub providers: HashMap<String, GatewayModelProvider>,
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
    /// determinism). App-subscription providers ([`APP_PROVIDERS`]) are only used
    /// when nothing else serves the model, so a route never points at Cursor or
    /// Copilot when a real API provider is available. Returns `None` for a model
    /// with neither an alias nor a provider.
    pub fn route_identifier(&self) -> Option<String> {
        if let Some(alias) = self.aliases.first() {
            return Some(alias.clone());
        }
        let mut providers: Vec<&String> = self.providers.keys().collect();
        providers.sort_by_key(|p| (is_app_provider(p), p.to_string()));
        providers
            .first()
            .map(|p| format!("{}/{}", p, self.model_id))
    }

    /// True when the model is served *only* through a coding-app subscription
    /// (Cursor, GitHub Copilot) — e.g. `cursor/composer-2`. Such a model is
    /// unreachable as a fallback/reroute target, so the settings pickers hide it.
    pub fn app_subscription_only(&self) -> bool {
        !self.providers.is_empty() && self.providers.keys().all(|p| is_app_provider(p))
    }

    /// `<author_id>/<model_id>` — the id used by the gateway's OpenAI-style
    /// `/v1/models` listing, so it joins this console catalog entry to the model
    /// ids agent configs are built from. `None` when `author_id` is absent.
    pub fn catalog_id(&self) -> Option<String> {
        if self.author_id.is_empty() {
            return None;
        }
        Some(format!("{}/{}", self.author_id, self.model_id))
    }

    /// The provider entry whose numbers describe this model. Context window and
    /// rates are both read from this single entry so they always describe the
    /// same upstream rather than being mixed across providers.
    ///
    /// Prefers the author's own entry — the native window, most accurate for an
    /// `<author>/<model>` route. Otherwise takes the smallest declared window,
    /// since overstating the window is the harmful direction: an agent that
    /// thinks it has more room than the upstream allows compacts too late and the
    /// request is rejected. Ties break on provider name, because `providers` is a
    /// `HashMap` and an arbitrary winner would make the emitted rates unstable.
    fn preferred_provider(&self) -> Option<&GatewayModelProvider> {
        if let Some(native) = self
            .providers
            .get(&self.author_id)
            .filter(|p| p.context_max_size > 0)
        {
            return Some(native);
        }
        let smallest_window = self
            .providers
            .iter()
            .filter(|(_, p)| p.context_max_size > 0)
            .min_by(|(a_name, a), (b_name, b)| {
                a.context_max_size
                    .cmp(&b.context_max_size)
                    .then_with(|| a_name.cmp(b_name))
            })
            .map(|(_, p)| p);
        if smallest_window.is_some() {
            return smallest_window;
        }
        // No provider declares a window, but its rates may still be known.
        self.providers
            .iter()
            .min_by(|(a_name, _), (b_name, _)| a_name.cmp(b_name))
            .map(|(_, p)| p)
    }

    /// The context window to advertise for this model, in tokens. `None` when no
    /// provider declares one.
    pub fn context_limit(&self) -> Option<u64> {
        self.preferred_provider()
            .map(|p| p.context_max_size)
            .filter(|c| *c > 0)
    }

    /// The per-million-token rates to advertise for this model. `None` only when
    /// the model has no provider entries at all; a model that is genuinely free
    /// yields zeroed rates.
    pub fn cost(&self) -> Option<GatewayModelCost> {
        self.preferred_provider().map(|p| GatewayModelCost {
            input: p.input_token_cost_per_million,
            output: p.output_token_cost_per_million,
            cache_read: p.cached_input_token_cost_per_million,
            cache_write: p.cache_creation_input_token_cost_per_million,
        })
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
    /// Output brevity (prompt nudging the model toward shorter responses): count of
    /// requests with brevity enabled and the summed assumed output-reduction
    /// fraction. There's no counterfactual for what a response would have been
    /// without the nudge, so this is estimated, not measured — average rate =
    /// `total_brevity_rate / total_brevity_requests`. Absent on session logs
    /// stored before this field was added, hence the lenient default.
    #[serde(default)]
    pub total_brevity_requests: u64,
    #[serde(default)]
    pub total_brevity_rate: f64,
    /// Tool surface reduction (MCP tool catalog consolidated into a single virtual
    /// search tool): tools-block token size before/after. Absent on session logs
    /// stored before this field was added, hence the lenient default.
    #[serde(default)]
    pub total_mcp_surface_tokens_before: u64,
    #[serde(default)]
    pub total_mcp_surface_tokens_after: u64,
}

impl SessionStats {
    /// Whether the session actually recorded traffic. A launch that exits
    /// without sending a single request through the gateway reports all zeroes,
    /// and there is nothing worth printing for it.
    pub fn has_activity(&self) -> bool {
        self.total_requests > 0
            || self.total_input_tokens > 0
            || self.total_output_tokens > 0
            || self.total_errors > 0
    }
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

        let status = resp.status();
        if status.is_success() {
            return resp.json().await.context("Invalid API key response");
        }
        // Surface the server's explanation (e.g. an unsupported coding_assistant)
        // instead of a bare status code.
        let text = resp.text().await.unwrap_or_default();
        let server_msg = serde_json::from_str::<ErrorEnvelope>(&text)
            .ok()
            .and_then(|e| e.describe());
        match (status.as_u16(), server_msg) {
            (401, _) => {
                anyhow::bail!("Authentication expired. Please run `edgee auth login` again.")
            }
            (_, Some(msg)) => anyhow::bail!("{msg}"),
            (s, None) => anyhow::bail!("Failed to get or create API key: HTTP {s}"),
        }
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

    /// Closes the session and returns its stats.
    ///
    /// `None` means the gateway never saw this session (404) — the agent ran but
    /// no request went through Edgee — as opposed to an error, which means we
    /// simply could not find out.
    pub async fn get_session_stats(
        &self,
        org_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionStats>> {
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
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        check_status(&resp, "get session stats")?;
        resp.json()
            .await
            .map(Some)
            .context("Invalid session stats response")
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

    fn catalog_model(json: &str) -> GatewayModel {
        serde_json::from_str(json).unwrap()
    }

    fn stats(json: &str) -> SessionStats {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn empty_session_has_no_activity() {
        let zeroes = r#"{"total_requests":0,"total_cost":0,"total_input_tokens":0,
            "total_output_tokens":0,"total_cached_input_tokens":0,
            "total_cache_creation_input_tokens":0,"total_reasoning_output_tokens":0,
            "total_token_cost_savings":0,"total_errors":0,
            "total_uncompressed_tools_tokens":0,"total_compressed_tools_tokens":0,
            "tool_compression_stats":null}"#;
        assert!(!stats(zeroes).has_activity());

        // A session that only errored still happened, and is worth reporting.
        let mut errored = stats(zeroes);
        errored.total_errors = 1;
        assert!(errored.has_activity());

        let mut used = stats(zeroes);
        used.total_requests = 1;
        assert!(used.has_activity());
    }

    #[test]
    fn catalog_id_joins_author_and_model() {
        let m = catalog_model(r#"{"model_id":"claude-opus-5","author_id":"anthropic"}"#);
        assert_eq!(m.catalog_id().as_deref(), Some("anthropic/claude-opus-5"));
        // No author means no gateway-listing id to join on.
        assert!(catalog_model(r#"{"model_id":"m1"}"#).catalog_id().is_none());
    }

    #[test]
    fn app_subscription_only_flags_cursor_and_copilot_exclusives() {
        let only_cursor = catalog_model(r#"{"model_id":"composer-2","providers":{"cursor":{}}}"#);
        assert!(only_cursor.app_subscription_only());

        let only_apps = catalog_model(
            r#"{"model_id":"claude-opus-4-8-fast","providers":{
                 "cursor":{},"github_copilot":{}}}"#,
        );
        assert!(only_apps.app_subscription_only());

        // A real API provider alongside them keeps the model routable.
        let mixed =
            catalog_model(r#"{"model_id":"gpt-5.4","providers":{"cursor":{},"openai":{}}}"#);
        assert!(!mixed.app_subscription_only());

        // No providers declared is not an app-only model.
        assert!(!catalog_model(r#"{"model_id":"m1"}"#).app_subscription_only());
    }

    #[test]
    fn context_limit_prefers_the_authors_own_provider() {
        // Cursor serves this model with a larger window than Anthropic does; the
        // author's native entry still wins for an `anthropic/...` route.
        let m = catalog_model(
            r#"{"model_id":"claude-haiku-4-5","author_id":"anthropic","providers":{
                 "anthropic":{"context_max_size":200000},
                 "cursor":{"context_max_size":256000}}}"#,
        );
        assert_eq!(m.context_limit(), Some(200_000));
    }

    #[test]
    fn context_limit_falls_back_to_the_smallest_non_zero_window() {
        // Served only via third parties (no `meta` provider entry): take the
        // smallest declared window rather than overstating it.
        let m = catalog_model(
            r#"{"model_id":"llama-3-3-70b-instruct","author_id":"meta","providers":{
                 "bedrock_us-east-1":{"context_max_size":128000},
                 "bedrock_eu-west-3":{"context_max_size":8192},
                 "vertex":{"context_max_size":0}}}"#,
        );
        assert_eq!(m.context_limit(), Some(8192));
    }

    #[test]
    fn cost_comes_from_the_same_provider_as_the_context_window() {
        // Cursor declares the smaller window but different rates; both numbers must
        // describe Anthropic's entry, not a mix of the two.
        let m = catalog_model(
            r#"{"model_id":"claude-sonnet-4-5","author_id":"anthropic","providers":{
                 "anthropic":{"context_max_size":1000000,
                   "input_token_cost_per_million":3,"output_token_cost_per_million":15,
                   "cached_input_token_cost_per_million":0.3,
                   "cache_creation_input_token_cost_per_million":3.75},
                 "cursor":{"context_max_size":256000,
                   "input_token_cost_per_million":99,"output_token_cost_per_million":99}}}"#,
        );
        assert_eq!(m.context_limit(), Some(1_000_000));
        assert_eq!(
            m.cost(),
            Some(GatewayModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
            })
        );
    }

    #[test]
    fn cost_is_zeroed_for_a_free_model() {
        // The server omits zero rates, so absent fields mean free, not unknown.
        let m = catalog_model(
            r#"{"model_id":"glm-4.5-flash","author_id":"zai","providers":{
                 "zai":{"context_max_size":128000}}}"#,
        );
        assert_eq!(
            m.cost(),
            Some(GatewayModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            })
        );
    }

    #[test]
    fn cost_is_known_even_when_no_provider_declares_a_window() {
        let m = catalog_model(
            r#"{"model_id":"m1","author_id":"acme","providers":{
                 "acme":{"input_token_cost_per_million":7}}}"#,
        );
        assert_eq!(m.context_limit(), None);
        assert_eq!(m.cost().map(|c| c.input), Some(7.0));
    }

    #[test]
    fn provider_selection_breaks_ties_on_name_for_stable_rates() {
        // Equal windows: the winner must not depend on HashMap iteration order.
        let m = catalog_model(
            r#"{"model_id":"m1","author_id":"acme","providers":{
                 "zeta":{"context_max_size":128000,"input_token_cost_per_million":9},
                 "alpha":{"context_max_size":128000,"input_token_cost_per_million":1}}}"#,
        );
        for _ in 0..16 {
            assert_eq!(m.cost().map(|c| c.input), Some(1.0));
        }
    }

    #[test]
    fn cost_is_absent_only_when_there_are_no_providers() {
        assert_eq!(catalog_model(r#"{"model_id":"m1"}"#).cost(), None);
    }

    #[test]
    fn context_limit_is_absent_when_no_provider_declares_one() {
        let m = catalog_model(
            r#"{"model_id":"m1","author_id":"acme","providers":{"acme":{"context_max_size":0}}}"#,
        );
        assert_eq!(m.context_limit(), None);
        assert_eq!(catalog_model(r#"{"model_id":"m1"}"#).context_limit(), None);
    }

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
