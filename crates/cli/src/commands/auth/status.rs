use std::collections::BTreeMap;

use anyhow::Result;
use console::style;
use serde::Serialize;

use crate::config::{self, ProviderConfig};

setup_command! {
    /// Emit machine-readable JSON instead of the human-readable summary.
    #[arg(long)]
    pub json: bool,
}

/// Per-provider auth summary in the `--json` output.
#[derive(Serialize)]
struct ProviderStatus {
    configured: bool,
    /// Connection mode ("plan" | "api") when the provider records one.
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
}

/// Machine-readable shape of `edgee auth status --json`. Consumed by external
/// front-ends (e.g. the macOS menubar app) so they don't scrape human output.
#[derive(Serialize)]
struct AuthStatusJson {
    logged_in: bool,
    profile: String,
    config_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_slug: Option<String>,
    providers: BTreeMap<String, ProviderStatus>,
}

/// The named coding-agent providers surfaced by `auth status`, in display order.
const PROVIDERS: &[(&str, &str)] = &[
    ("claude", "Claude"),
    ("codex", "Codex"),
    ("opencode", "OpenCode"),
    ("crush", "Crush"),
];

fn provider_config<'a>(creds: &'a config::Credentials, key: &str) -> Option<&'a ProviderConfig> {
    match key {
        "claude" => creds.claude.as_ref(),
        "codex" => creds.codex.as_ref(),
        "opencode" => creds.opencode.as_ref(),
        "crush" => creds.crush.as_ref(),
        _ => None,
    }
}

fn is_configured(provider: Option<&ProviderConfig>) -> bool {
    provider.map(|p| !p.api_key.is_empty()).unwrap_or(false)
}

pub async fn run(opts: Options) -> Result<()> {
    let creds = config::read()?;

    let has_token = creds
        .user_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .is_some();
    let any_provider = PROVIDERS
        .iter()
        .any(|(key, _)| is_configured(provider_config(&creds, key)));
    let logged_in = has_token || any_provider;

    if opts.json {
        let providers = PROVIDERS
            .iter()
            .filter_map(|(key, _)| {
                let p = provider_config(&creds, key);
                if !is_configured(p) {
                    return None;
                }
                Some((
                    key.to_string(),
                    ProviderStatus {
                        configured: true,
                        mode: p.and_then(|p| p.connection.clone()),
                    },
                ))
            })
            .collect();

        let status = AuthStatusJson {
            logged_in,
            profile: config::active_profile_name(),
            config_path: config::credentials_path().display().to_string(),
            email: creds.email.clone().filter(|e| !e.is_empty()),
            org_slug: creds.org_slug.clone().filter(|s| !s.is_empty()),
            providers,
        };
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    if !logged_in {
        println!(
            "\n  {} {}\n",
            style("✗").red().bold(),
            style("Not logged in. Run `edgee auth login` to authenticate.").dim()
        );
        return Ok(());
    }

    println!();
    println!(
        "   {}  {}",
        style("Config:").dim(),
        style(config::credentials_path().display()).dim()
    );
    println!(
        "   {}  {}",
        style("Profile:").dim(),
        style(config::active_profile_name()).bold()
    );

    match &creds.email {
        Some(e) if !e.is_empty() => println!(
            "\n  {} {}",
            style("✓").green().bold(),
            style(format!("Logged in as {e}")).bold()
        ),
        _ => println!(
            "\n  {} {}",
            style("✓").green().bold(),
            style("Logged in").bold()
        ),
    }

    for (key, name) in PROVIDERS {
        let provider = provider_config(&creds, key);
        if let Some(p) = provider.filter(|p| !p.api_key.is_empty()) {
            println!(
                "   {}  {}",
                style(format!("{name}:")).dim(),
                style("configured").green()
            );
            if let Some(mode) = &p.connection {
                println!(
                    "   {}  {}",
                    style(format!("{name} mode:")).dim(),
                    style(mode).cyan()
                );
            }
        }
    }
    println!();

    Ok(())
}
