use anyhow::{Context, Result};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    api::{ApiClient, GatewayModel},
    config::Credentials,
};

/// Launch-level flags deliberately stay off the targets so agent flags pass through.
#[derive(Debug, Default, clap::Args)]
#[group(id = "session_reroute", multiple = true)]
pub struct Options {
    /// Reroute this session to a model (place before the agent name)
    #[arg(long = "reroute", value_name = "MODEL")]
    pub target: Option<String>,
    /// Only reroute this source model (default: all models)
    #[arg(long = "reroute-from", requires = "target", value_name = "MODEL")]
    source: Option<String>,
    /// Pin the catalog provider serving the target model
    #[arg(
        long = "reroute-provider",
        requires = "target",
        value_name = "PROVIDER"
    )]
    provider: Option<String>,
    /// Reasoning effort for the target model
    #[arg(long = "reroute-effort", requires = "target", value_name = "EFFORT")]
    effort: Option<String>,
    /// Reroute lifetime in minutes (default: 60; maximum: 1440)
    #[arg(
        long = "reroute-duration",
        requires = "target",
        value_name = "MINUTES",
        value_parser = clap::value_parser!(u16).range(1..=1440)
    )]
    duration: Option<u16>,
}

/// A reroute validated against the catalog, ready to attach to a launch session.
#[derive(Debug, Default)]
pub struct Reroute(Option<Validated>);

#[derive(Debug)]
struct Validated {
    source: String,
    target: String,
    provider: Option<String>,
    effort: Option<String>,
    duration: u16,
}

impl Options {
    /// Sign in and validate the reroute before any agent setup, so a bad model
    /// fails fast instead of after onboarding and agent configuration prompts.
    pub async fn resolve(&self) -> Result<Reroute> {
        if self.target.is_none() {
            return Ok(Reroute::default());
        }
        let creds = crate::config::read()?;
        if creds.user_token.as_deref().unwrap_or("").is_empty() {
            crate::commands::auth::login::perform_login().await?;
        }
        crate::commands::auth::login::ensure_org_selected().await?;
        let creds = crate::config::read()?;
        let token = creds
            .user_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .context("Sign in with `edgee auth login` before using --reroute")?;
        let org_id = creds
            .org_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .context("Select an organization with `edgee auth login` before using --reroute")?;
        self.validate(&ApiClient::new(token)?, org_id).await
    }

    async fn validate(&self, client: &ApiClient, org_id: &str) -> Result<Reroute> {
        let models = client.list_available_models(org_id).await.context(
            "Agent was not launched. Could not validate the reroute catalog; retry when the API is available",
        )?;
        let (target, effort) = self.validate_target(&models)?;
        let source = self.source.as_deref().unwrap_or("*").trim();
        let source = if source == "*" {
            "*".to_string()
        } else {
            // A source may itself be disabled as a destination by org policy.
            let catalog = client
                .list_models()
                .await
                .context("Could not validate the source model; retry")?;
            find_model(&catalog, source)
                .and_then(GatewayModel::catalog_id)
                .with_context(|| {
                    format!("Unknown source model '{source}'. Choose a catalog model or use --reroute-from '*'.")
                })?
        };
        Ok(Reroute(Some(Validated {
            source,
            target,
            provider: self.provider.as_deref().map(|p| p.trim().to_string()),
            effort,
            duration: self.duration.unwrap_or(60),
        })))
    }

    fn validate_target(&self, models: &[GatewayModel]) -> Result<(String, Option<String>)> {
        let target = self
            .target
            .as_deref()
            .context("Specify --reroute MODEL")?
            .trim();
        let model = find_model(models, target).with_context(|| {
            format!("Reroute target '{target}' is not available in your organization's catalog. Choose an available model.")
        })?;
        anyhow::ensure!(
            model.active && !model.providers.is_empty() && !model.app_subscription_only(),
            "Model '{target}' cannot be used as a reroute target. Choose an active API model."
        );
        if let Some(provider) = self.provider.as_deref().map(str::trim) {
            anyhow::ensure!(
                model.providers.contains_key(provider) && !crate::api::is_app_provider(provider),
                "Provider '{provider}' is not available for '{target}'. Choose a provider from its catalog entry or omit --reroute-provider."
            );
        }
        let effort = self
            .effort
            .as_deref()
            .map(|s| s.trim().to_ascii_lowercase());
        if let Some(effort) = effort.as_deref() {
            // Match the API: an empty effort list imposes no model-specific restriction.
            anyhow::ensure!(
                ["none", "minimal", "low", "medium", "high", "xhigh", "max"].contains(&effort)
                    && (model.reasoning_efforts.is_empty()
                        || model.reasoning_efforts.iter().any(|e| e == effort)),
                "Effort '{effort}' is not supported for '{target}'. Choose a catalog effort or omit --reroute-effort."
            );
        }
        let id = model
            .catalog_id()
            .context("Catalog model has no author ID; choose another model")?;
        Ok((id, effort))
    }
}

impl Reroute {
    /// Reserve the exact session ID that the launcher will attach to requests.
    /// An explicitly requested reroute must succeed before the agent can start.
    pub async fn create_session(&self, creds: &Credentials, provider: &str) -> Result<String> {
        if self.0.is_none() {
            return Ok(uuid::Uuid::new_v4().to_string());
        }
        let token = creds
            .user_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .context("Sign in with `edgee auth login` before using --reroute")?;
        let org_id = creds
            .org_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .context("Select an organization with `edgee auth login` before using --reroute")?;
        let key_id = creds
            .provider(provider)
            .and_then(|p| p.api_key_id.as_deref())
            .filter(|s| !s.is_empty())
            .context("Missing agent API key ID; run `edgee auth login` and retry")?;
        self.reserve(&ApiClient::new(token)?, org_id, key_id).await
    }

    async fn reserve(&self, client: &ApiClient, org_id: &str, key_id: &str) -> Result<String> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let Some(reroute) = &self.0 else {
            return Ok(session_id);
        };
        let expires_at = (OffsetDateTime::now_utc()
            + time::Duration::minutes(i64::from(reroute.duration)))
        .format(&Rfc3339)?;
        let request = crate::api::SessionRerouteRequest {
            api_key_id: key_id,
            source_model: &reroute.source,
            target_model: &reroute.target,
            target_provider: reroute.provider.as_deref(),
            effort: reroute.effort.as_deref(),
            expires_at: &expires_at,
        };
        client
            .put_session_reroute(org_id, &session_id, &request)
            .await
            .context(
                "Agent was not launched. Check the reroute model and your access, then retry",
            )?;
        eprintln!(
            "Session reroute to {} enabled for {} minutes.",
            reroute.target, reroute.duration
        );
        Ok(session_id)
    }
}

fn find_model<'a>(models: &'a [GatewayModel], name: &str) -> Option<&'a GatewayModel> {
    models
        .iter()
        .find(|m| m.catalog_id().as_deref() == Some(name))
        .or_else(|| models.iter().find(|m| m.aliases.iter().any(|a| a == name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_json() -> serde_json::Value {
        serde_json::json!([{
            "author_id": "openai", "model_id": "gpt-5", "active": true,
            "aliases": ["gpt-5"], "providers": {"openai": {}},
            "reasoning_efforts": ["low", "high"]
        }])
    }

    fn catalog() -> Vec<GatewayModel> {
        serde_json::from_value(catalog_json()).unwrap()
    }

    #[test]
    fn validates_alias_provider_and_effort() {
        let opts = Options {
            target: Some("gpt-5".into()),
            provider: Some("openai".into()),
            effort: Some("LOW".into()),
            ..Default::default()
        };
        assert_eq!(
            opts.validate_target(&catalog()).unwrap(),
            ("openai/gpt-5".into(), Some("low".into()))
        );
        assert!(find_model(&catalog(), "unknown").is_none());
    }

    #[test]
    fn rejects_unavailable_models_providers_and_efforts() {
        for (target, provider, effort) in [
            ("missing", None, None),
            ("gpt-5", Some("bedrock"), None),
            ("gpt-5", None, Some("max")),
            ("gpt-5", None, Some("typo")),
        ] {
            let opts = Options {
                target: Some(target.into()),
                provider: provider.map(str::to_string),
                effort: effort.map(str::to_string),
                ..Default::default()
            };
            assert!(opts.validate_target(&catalog()).is_err());
        }
        let opts = Options {
            target: Some("gpt-5".into()),
            ..Default::default()
        };
        assert!(opts.validate_target(&[]).is_err());
        let mut models = catalog();
        models[0].active = false;
        assert!(opts.validate_target(&models).is_err());
        models[0].active = true;
        let provider = models[0].providers.remove("openai").unwrap();
        models[0]
            .providers
            .insert("github_copilot".into(), provider);
        assert!(opts.validate_target(&models).is_err());
    }

    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>;

    /// Minimal console API: answers the first route whose `"METHOD /path"` prefixes
    /// the request line (404 otherwise) and records each request line and body.
    async fn mock_api(
        routes: Vec<(&'static str, &'static str, serde_json::Value)>,
    ) -> (String, Seen) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Seen::default();
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                let (head, body) = loop {
                    let mut buf = [0; 4096];
                    let count = stream.read(&mut buf).await.unwrap();
                    data.extend_from_slice(&buf[..count]);
                    let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8(data[..end].to_vec()).unwrap();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        let body = serde_json::from_slice(&data[end + 4..end + 4 + length])
                            .unwrap_or(serde_json::Value::Null);
                        break (head, body);
                    }
                };
                let line = head.lines().next().unwrap().trim_end_matches(" HTTP/1.1");
                let (status, reply) = routes
                    .iter()
                    .find(|(route, _, _)| line.starts_with(route))
                    .map(|(_, status, reply)| (*status, reply.to_string()))
                    .unwrap_or(("404 Not Found", "{}".into()));
                log.lock().unwrap().push((line.to_string(), body));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (base_url, seen)
    }

    #[tokio::test]
    async fn reroute_from_resolves_source_against_full_catalog() {
        let sonnet = serde_json::json!([{
            "author_id": "anthropic", "model_id": "claude-sonnet", "active": true,
            "aliases": ["sonnet"], "providers": {"anthropic": {}}
        }]);
        let (base_url, seen) = mock_api(vec![
            (
                "GET /v1/organizations/org/available-models",
                "200 OK",
                catalog_json(),
            ),
            ("GET /v1/models", "200 OK", sonnet),
        ])
        .await;
        let client = ApiClient::with_base_url("token", &base_url).unwrap();
        let opts = Options {
            target: Some("openai/gpt-5".into()),
            source: Some("sonnet".into()),
            ..Default::default()
        };
        let Reroute(Some(reroute)) = opts.validate(&client, "org").await.unwrap() else {
            panic!("reroute not validated")
        };
        assert_eq!(reroute.source, "anthropic/claude-sonnet");
        let lines: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(l, _)| l.clone())
            .collect();
        assert_eq!(
            lines,
            [
                "GET /v1/organizations/org/available-models",
                "GET /v1/models"
            ]
        );

        let opts = Options {
            source: Some("unknown".into()),
            ..opts
        };
        let err = opts.validate(&client, "org").await.unwrap_err();
        assert!(err.to_string().contains("Unknown source model 'unknown'"));
    }

    fn validated() -> Reroute {
        Reroute(Some(Validated {
            source: "*".into(),
            target: "openai/gpt-5".into(),
            provider: Some("openai".into()),
            effort: Some("low".into()),
            duration: 60,
        }))
    }

    #[tokio::test]
    async fn reserve_puts_the_session_id_it_returns() {
        let (base_url, seen) = mock_api(vec![(
            "PUT /v1/organizations/org/sessions/",
            "200 OK",
            serde_json::json!({}),
        )])
        .await;
        let client = ApiClient::with_base_url("token", &base_url).unwrap();
        let session = validated().reserve(&client, "org", "key").await.unwrap();

        let seen = seen.lock().unwrap();
        let [(line, body)] = seen.as_slice() else {
            panic!("expected one request, got {seen:?}")
        };
        assert_eq!(
            line,
            &format!("PUT /v1/organizations/org/sessions/{session}/reroute")
        );
        assert_eq!(body["api_key_id"], "key");
        assert_eq!(body["source_model"], "*");
        assert_eq!(body["target_model"], "openai/gpt-5");
        assert_eq!(body["target_provider"], "openai");
        assert_eq!(body["effort"], "low");
    }

    #[tokio::test]
    async fn rejected_reroute_blocks_the_launch() {
        let (base_url, _) = mock_api(vec![(
            "PUT /v1/organizations/org/sessions/",
            "403 Forbidden",
            serde_json::json!({}),
        )])
        .await;
        let client = ApiClient::with_base_url("token", &base_url).unwrap();
        let err = validated()
            .reserve(&client, "org", "key")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Agent was not launched"));
    }

    #[tokio::test]
    async fn ordinary_launch_needs_no_reroute_credentials() {
        let reroute = Options::default().resolve().await.unwrap();
        let session = reroute
            .create_session(&Credentials::default(), "claude")
            .await
            .unwrap();
        assert!(uuid::Uuid::parse_str(&session).is_ok());
    }

    #[tokio::test]
    async fn requested_reroute_fails_without_credentials() {
        let reroute = validated();
        assert!(reroute
            .create_session(&Credentials::default(), "claude")
            .await
            .is_err());
        let creds = Credentials {
            user_token: Some("token".into()),
            org_id: Some("org".into()),
            ..Default::default()
        };
        assert!(reroute
            .create_session(&creds, "claude")
            .await
            .unwrap_err()
            .to_string()
            .contains("API key ID"));
    }
}
