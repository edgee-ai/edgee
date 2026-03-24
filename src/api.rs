use anyhow::{Context, Result};
use serde::Deserialize;
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
        let resp = self.http.get(&url).send().await.context("Failed to list organizations")?;
        check_status(&resp, "list organizations")?;
        let body: ListResponse<Organization> = resp.json().await.context("Invalid organization response")?;
        Ok(body.data)
    }

    pub async fn get_or_create_key(&self, org_id: &str, coding_assistant: &str) -> Result<ApiKeyItem> {
        let url = format!("{}/v1/organizations/{}/api_keys/get-or-create", self.base_url, org_id);
        let body = serde_json::json!({ "coding_assistant": coding_assistant, "compression": true });
        let resp = self.http.post(&url).json(&body).send().await.context("Failed to get or create API key")?;
        check_status(&resp, "get or create API key")?;
        resp.json().await.context("Invalid API key response")
    }
}

fn check_status(resp: &reqwest::Response, action: &str) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    match status.as_u16() {
        401 => anyhow::bail!("Authentication expired. Please run `edgee auth login` again."),
        403 => anyhow::bail!("Permission denied: you don't have access to {action} on this organization."),
        404 => anyhow::bail!("Not found while trying to {action}. The resource may have been deleted."),
        _ => anyhow::bail!("Failed to {action}: HTTP {status}"),
    }
}
