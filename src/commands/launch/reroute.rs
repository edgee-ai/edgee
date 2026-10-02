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
        let client = ApiClient::new(token)?;
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
        let session_id = uuid::Uuid::new_v4().to_string();
        let Some(reroute) = &self.0 else {
            return Ok(session_id);
        };
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
        ApiClient::new(token)?
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

    fn catalog() -> Vec<GatewayModel> {
        serde_json::from_value(serde_json::json!([{
            "author_id": "openai", "model_id": "gpt-5", "active": true,
            "aliases": ["gpt-5"], "providers": {"openai": {}},
            "reasoning_efforts": ["low", "high"]
        }]))
        .unwrap()
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
        let reroute = Reroute(Some(Validated {
            source: "*".into(),
            target: "openai/gpt-5".into(),
            provider: None,
            effort: None,
            duration: 60,
        }));
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
