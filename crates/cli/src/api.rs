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

#[derive(Deserialize)]
pub struct ApiKeyItem {
    pub id: String,
    pub key: Option<String>,
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
