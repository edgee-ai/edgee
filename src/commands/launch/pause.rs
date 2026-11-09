use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{api::ApiClient, config::Credentials};

pub const HEADER_NAMES: [&str; 3] = [
    "x-edgee-session-control-incarnation",
    "x-edgee-session-control-revision",
    "x-edgee-session-control-until",
];
pub const HEADER_ENVS: [&str; 3] = [
    "EDGEE_SESSION_CONTROL_INCARNATION",
    "EDGEE_SESSION_CONTROL_REVISION",
    "EDGEE_SESSION_CONTROL_UNTIL",
];

#[derive(Debug, Default, clap::Args)]
#[group(id = "optimization_pause", multiple = true)]
pub struct Options {
    /// Pause session optimizations; traffic and security remain through Edgee
    #[arg(long)]
    pub pause_optimizations: bool,
    /// Pause lifetime in minutes (default: 60; maximum: 1440)
    #[arg(long, requires = "pause_optimizations", value_name = "MINUTES", value_parser = clap::value_parser!(u16).range(1..=1440))]
    pub pause_duration: Option<u16>,
}

#[derive(Debug, Default)]
pub struct Controls {
    pub reroute: super::reroute::Reroute,
    pub pause: Options,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Requirement {
    pub api_key_id: String,
    pub scope_incarnation: String,
    pub revision: u64,
    pub valid_until: i64,
}

impl Requirement {
    pub fn headers(&self) -> [(&'static str, String); 3] {
        [
            (HEADER_NAMES[0], self.scope_incarnation.clone()),
            (HEADER_NAMES[1], self.revision.to_string()),
            (HEADER_NAMES[2], self.valid_until.to_string()),
        ]
    }

    pub fn header_map(&self) -> Result<http::HeaderMap> {
        let mut headers = http::HeaderMap::with_capacity(3);
        for (name, value) in self.headers() {
            headers.insert(name, http::HeaderValue::from_str(&value)?);
        }
        Ok(headers)
    }
}

pub fn line_headers(requirement: Option<&Requirement>) -> String {
    requirement.map(|r| r.headers().into_iter().map(|(k, v)| format!("\n{k}: {v}")).collect()).unwrap_or_default()
}

pub fn toml_headers(requirement: Option<&Requirement>) -> String {
    requirement.map(|r| r.headers().into_iter().map(|(k, v)| format!(",\"{k}\"=\"{v}\"")).collect()).unwrap_or_default()
}

pub fn insert_headers(headers: &mut serde_json::Value, requirement: Option<&Requirement>) {
    if let Some(requirement) = requirement {
        for (name, value) in requirement.headers() {
            headers[name] = serde_json::Value::String(value);
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Status {
    pub routing_session_id: String,
    pub launch_id: Option<String>,
    pub scope_incarnation: Option<String>,
    pub scope_revision: u64,
    pub registered_api_key_ids: Vec<String>,
    pub selected_api_key_ids: Vec<String>,
    pub scope_complete: bool,
    #[serde(default)]
    pub requirements: Vec<Requirement>,
}

#[derive(Serialize)]
pub struct Request<'a> {
    pub api_key_ids: &'a [String],
    pub expires_at: &'a str,
    pub expected_scope_incarnation: Option<&'a str>,
    pub expected_scope_revision: u64,
    pub operation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    protocol_version: u64,
    session_revision_requirement: bool,
}

async fn probe(client: &reqwest::Client, gateway_url: &str, api_key: &str) -> Result<()> {
    let url = crate::config::url_with_path(gateway_url, &["v1", "optimization-pause", "capabilities"])?;
    let response = client.get(url).bearer_auth(api_key).send().await?
        .error_for_status().context("Gateway does not support paused launches, or rejected the installed key")?;
    anyhow::ensure!(response.status().is_success(), "Gateway pause capability endpoint did not return success");
    let capabilities: Capabilities = response.json().await.context("Unsupported gateway pause capability response; upgrade every serving instance")?;
    anyhow::ensure!(capabilities.protocol_version == 1 && capabilities.session_revision_requirement,
        "Gateway does not enforce session revision requirements; upgrade every serving instance before using paused launches");
    Ok(())
}

impl Controls {
    pub async fn create_session(&self, creds: &Credentials, provider: &str, gateway_url: &str) -> Result<(String, Option<Requirement>)> {
        let (session_id, mut requirements) = self.create_session_for_keys(creds, provider, &[provider], gateway_url).await?;
        let key = creds.provider_api_key(provider).context("Missing launch API key")?;
        let requirement = requirements.remove(key);
        anyhow::ensure!(!self.pause.pause_optimizations || requirement.is_some(), "Agent was not launched. Missing pause requirement for its installed key");
        Ok((session_id, requirement))
    }

    pub async fn create_session_for_keys(&self, creds: &Credentials, primary: &str, providers: &[&str], gateway_url: &str) -> Result<(String, HashMap<String, Requirement>)> {
        if !self.pause.pause_optimizations {
            let session_id = self.reroute.create_session(creds, primary).await?;
            if !self.reroute.is_active() {
                super::util::report_cli_session(creds, &session_id, primary).await;
            }
            return Ok((session_id, HashMap::new()));
        }
        anyhow::ensure!(!self.reroute.is_active(), "--pause-optimizations cannot be combined with --reroute");
        let token = creds.user_token.as_deref().filter(|s| !s.is_empty()).context("Sign in with `edgee auth login` before pausing optimizations")?;
        let org = creds.org_id.as_deref().filter(|s| !s.is_empty()).context("Select an organization before pausing optimizations")?;
        let mut keys = Vec::new();
        let mut ids = Vec::new();
        for provider in providers {
            let key = creds.provider(provider).context("Missing installed agent key")?;
            let id = key.api_key_id.as_deref().filter(|s| !s.is_empty()).context("Missing agent API key ID; run `edgee auth login` and retry")?;
            anyhow::ensure!(!key.api_key.is_empty(), "Missing installed agent API key");
            if let Some(index) = ids.iter().position(|existing| existing == id) {
                anyhow::ensure!(keys[index] == key.api_key.as_str(), "Conflicting credentials for one installed API key ID");
            } else {
                ids.push(id.to_string());
                keys.push(key.api_key.as_str());
            }
        }
        anyhow::ensure!(!ids.is_empty() && ids.len() <= 16, "A pause requires between 1 and 16 installed keys");
        let primary_id = creds.provider(primary).and_then(|key| key.api_key_id.as_deref()).context("Missing primary launch key ID")?;
        anyhow::ensure!(ids.iter().any(|id| id == primary_id), "Pause scope omitted the primary launch key");
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build()?;
        for key in &keys {
            probe(&http, gateway_url, key).await.context("Agent was not launched. Gateway pause capability preflight failed")?;
        }
        let client = ApiClient::new(token)?;
        let (session_id, requirements) = reserve(&client, org, primary_id, &ids, self.pause.pause_duration.unwrap_or(60)).await?;
        let requirements = ids.into_iter().zip(keys).map(|(id, key)| (key.to_string(), requirements[&id].clone())).collect();
        Ok((session_id, requirements))
    }
}

async fn reserve(client: &ApiClient, org: &str, primary_id: &str, ids: &[String], minutes: u16) -> Result<(String, HashMap<String, Requirement>)> {
    let session_id = uuid::Uuid::new_v4().to_string();
    client.set_session_cli_version(org, &session_id, env!("CARGO_PKG_VERSION"), Some(primary_id)).await.context("Agent was not launched. Primary launch registration failed")?;
    let requirements = publish(client, org, &session_id, ids, minutes).await?;
    Ok((session_id, requirements))
}

async fn publish(client: &ApiClient, org: &str, session_id: &str, ids: &[String], minutes: u16) -> Result<HashMap<String, Requirement>> {
    let state = client.get_optimization_pause(org, session_id, ids).await.context("Agent was not launched. Could not read the explicit pause scope")?;
    anyhow::ensure!(state.routing_session_id == session_id && state.launch_id.as_deref() == Some(session_id), "Pause API returned a different launch identity; agent was not launched");
    let deadline = OffsetDateTime::now_utc() + time::Duration::minutes(i64::from(minutes));
    let expires_at = deadline.format(&Rfc3339)?;
    eprintln!("Pausing optimizations until {expires_at}. Existing policy resumes then; traffic, security and usage remain through Edgee.");
    let request = Request {
        api_key_ids: ids, expires_at: &expires_at,
        expected_scope_incarnation: state.scope_incarnation.as_deref(),
        expected_scope_revision: state.scope_revision,
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let status = match client.put_optimization_pause(org, session_id, &request).await {
        Ok(status) => status,
        Err(error) => {
            // A failed response may still have committed. Never infer success or retry the mutation.
            let refreshed = client.get_optimization_pause(org, session_id, ids).await;
            let detail = match refreshed {
                Ok(state) => format!("authoritative scope revision is {}", state.scope_revision),
                Err(_) => "authoritative status could not be read".to_string(),
            };
            anyhow::bail!("Agent was not launched. Pause publication failed ({detail}); inspect the session in the console before retrying: {error:#}");
        }
    };
    validate_requirements(status, session_id, ids, deadline.unix_timestamp())
}

fn validate_requirements(status: Status, session_id: &str, ids: &[String], deadline: i64) -> Result<HashMap<String, Requirement>> {
    let expected: HashSet<&str> = ids.iter().map(String::as_str).collect();
    anyhow::ensure!(status.routing_session_id == session_id && status.launch_id.as_deref() == Some(session_id)
        && status.scope_complete && status.scope_revision > 0
        && status.registered_api_key_ids.len() == expected.len()
        && status.selected_api_key_ids.len() == expected.len()
        && status.registered_api_key_ids.iter().map(String::as_str).collect::<HashSet<_>>() == expected
        && status.selected_api_key_ids.iter().map(String::as_str).collect::<HashSet<_>>() == expected,
        "Agent was not launched. Pause API returned an incomplete or mismatched scope");
    let incarnation = status.scope_incarnation.context("Pause API omitted the scope incarnation")?;
    uuid::Uuid::parse_str(&incarnation).context("Pause API returned an invalid scope incarnation")?;
    let mut requirements = HashMap::new();
    for requirement in status.requirements {
        anyhow::ensure!(expected.contains(requirement.api_key_id.as_str()) && requirement.scope_incarnation == incarnation
            && requirement.revision > 0 && requirement.revision == status.scope_revision
            && requirement.valid_until == deadline,
            "Agent was not launched. Pause API returned an invalid per-key requirement");
        anyhow::ensure!(!requirements.contains_key(&requirement.api_key_id), "Pause API returned duplicate key requirements");
        requirements.insert(requirement.api_key_id.clone(), requirement);
    }
    anyhow::ensure!(requirements.len() == ids.len(), "Agent was not launched. Pause API omitted installed key requirements");
    Ok(requirements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const INCARNATION: &str = "672a7943-3648-46a5-a90f-17b356a13ac4";

    async fn server(
        count: usize,
        mut reply: impl FnMut(&str, &Value) -> (u16, Value) + Send + 'static,
    ) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for _ in 0..count {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                let (head, body) = loop {
                    let mut buf = [0; 4096];
                    let size = stream.read(&mut buf).await.unwrap();
                    assert_ne!(size, 0, "request ended early");
                    data.extend_from_slice(&buf[..size]);
                    let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                    let head = String::from_utf8(data[..end].to_vec()).unwrap();
                    let length = head.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    }).unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        let body = serde_json::from_slice(&data[end + 4..end + 4 + length]).unwrap_or(Value::Null);
                        break (head, body);
                    }
                };
                let (status, body_out) = reply(&head, &body);
                seen.push((head, body));
                let body_out = body_out.to_string();
                stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_out}", body_out.len()).as_bytes()).await.unwrap();
            }
            seen
        });
        (base, task)
    }

    fn status(session: &str, keys: &[String], until: Option<i64>) -> Value {
        json!({
            "routing_session_id": session, "launch_id": session,
            "scope_incarnation": until.map(|_| INCARNATION),
            "scope_revision": if until.is_some() { 1 } else { 0 },
            "registered_api_key_ids": keys, "selected_api_key_ids": keys,
            "scope_complete": true,
            "requirements": until.map(|until| keys.iter().map(|key| json!({
                "api_key_id": key, "scope_incarnation": INCARNATION, "revision": 1, "valid_until": until
            })).collect::<Vec<_>>()).unwrap_or_default()
        })
    }

    #[tokio::test]
    async fn pause_registers_primary_once_and_publishes_every_key_for_returned_identity() {
        let keys = vec!["copilot-id".to_string(), "claude-id".to_string()];
        let expected = keys.clone();
        let (base, task) = server(3, move |head, body| {
            let line = head.lines().next().unwrap();
            let path = line.split_whitespace().nth(1).unwrap();
            let session = path.split('/').nth(5).unwrap();
            if line.starts_with("POST ") {
                assert_eq!(body["api_key_id"], "copilot-id");
                (200, json!({}))
            } else if line.starts_with("GET ") {
                assert!(path.ends_with("?api_key_ids=copilot-id%2Cclaude-id"));
                (200, status(session, &expected, None))
            } else {
                assert_eq!(body["api_key_ids"], json!(expected));
                assert!(body["expected_scope_incarnation"].is_null());
                assert_eq!(body["expected_scope_revision"], 0);
                uuid::Uuid::parse_str(body["operation_id"].as_str().unwrap()).unwrap();
                let until = OffsetDateTime::parse(body["expires_at"].as_str().unwrap(), &Rfc3339).unwrap().unix_timestamp();
                (200, status(session, &expected, Some(until)))
            }
        }).await;
        let client = ApiClient::with_base_url("token", &base).unwrap();
        let (session, requirements) = reserve(&client, "org", "copilot-id", &keys, 60).await.unwrap();
        let seen = task.await.unwrap();
        assert!(seen[0].0.starts_with(&format!("POST /v1/organizations/org/launch-sessions/{session}/cli-version ")));
        assert!(seen[1].0.starts_with(&format!("GET /v1/organizations/org/launch-sessions/{session}/optimization-pause?")));
        assert!(seen[2].0.starts_with(&format!("PUT /v1/organizations/org/launch-sessions/{session}/optimization-pause ")));
        assert_eq!(requirements.keys().collect::<HashSet<_>>(), keys.iter().collect::<HashSet<_>>());
        assert_eq!(requirements["claude-id"].valid_until, requirements["copilot-id"].valid_until);
    }

    #[tokio::test]
    async fn publication_failure_refreshes_state_but_never_allows_launch_or_retries_write() {
        let (base, task) = server(4, |head, _| {
            let line = head.lines().next().unwrap();
            let session = line.split_whitespace().nth(1).unwrap().split('/').nth(5).unwrap();
            if line.starts_with("POST ") { (200, json!({})) }
            else if line.starts_with("PUT ") { (503, json!({"error": "publication failed"})) }
            else { (200, status(session, &["key".to_string()], Some(2_000_000_000))) }
        }).await;
        let client = ApiClient::with_base_url("token", &base).unwrap();
        assert!(reserve(&client, "org", "key", &["key".to_string()], 60).await.is_err());
        let seen = task.await.unwrap();
        assert_eq!(seen.iter().filter(|(head, _)| head.starts_with("PUT ")).count(), 1);
        assert!(seen[3].0.starts_with("GET "));
    }

    #[tokio::test]
    async fn capability_checks_each_key_at_resolved_prefix_and_rejects_mixed_support() {
        let (base, task) = server(2, |head, _| {
            assert!(head.starts_with("GET /tenant/gateway/v1/optimization-pause/capabilities "));
            let supported = head.to_ascii_lowercase().contains("authorization: bearer primary-key");
            (200, json!({"protocol_version": 1, "session_revision_requirement": supported}))
        }).await;
        let http = reqwest::Client::new();
        let url = format!("{base}/tenant/gateway");
        probe(&http, &url, "primary-key").await.unwrap();
        assert!(probe(&http, &url, "secondary-key").await.is_err());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn capability_rejects_legacy_auth_failure_malformed_and_future_protocols() {
        for (status_code, response) in [
            (404, json!({})), (401, json!({})), (200, json!({})),
            (200, json!({"protocol_version": 2, "session_revision_requirement": true})),
            (200, json!({"protocol_version": 1, "session_revision_requirement": true, "unknown": true})),
        ] {
            let (base, task) = server(1, move |_, _| (status_code, response.clone())).await;
            assert!(probe(&reqwest::Client::new(), &base, "key").await.is_err());
            task.await.unwrap();
        }
    }

    #[test]
    fn missing_secondary_wrong_identity_incarnation_or_deadline_never_admits_launch() {
        let keys = vec!["primary".to_string(), "secondary".to_string()];
        let good = status("session", &keys, Some(2_000_000_000));
        let mutations: [fn(&mut Value); 6] = [
            |v: &mut Value| { v["requirements"].as_array_mut().unwrap().pop(); },
            |v: &mut Value| { v["routing_session_id"] = json!("other"); },
            |v: &mut Value| { v["requirements"][1]["scope_incarnation"] = json!("9d739e1e-470e-42ac-a383-07c09a870f64"); },
            |v: &mut Value| { v["requirements"][1]["valid_until"] = json!(1_999_999_999); },
            |v: &mut Value| { v["scope_complete"] = json!(false); },
            |v: &mut Value| { v["requirements"][1]["api_key_id"] = json!("primary"); },
        ];
        for mutate in mutations {
            let mut bad = good.clone();
            mutate(&mut bad);
            assert!(validate_requirements(serde_json::from_value(bad).unwrap(), "session", &keys, 2_000_000_000).is_err());
        }
        assert!(validate_requirements(serde_json::from_value(good).unwrap(), "session", &keys, 2_000_000_000).is_ok());
    }
}
