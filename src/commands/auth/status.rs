use std::collections::BTreeMap;

use anyhow::Result;
use console::style;
use serde::Serialize;

use crate::commands::util;
use crate::config;

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

pub async fn run(opts: Options) -> Result<()> {
    let creds = config::read()?;

    let has_token = creds
        .user_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .is_some();
    let any_provider = PROVIDERS
        .iter()
        .any(|(key, _)| creds.provider_configured(key));
    let logged_in = has_token || any_provider;

    if opts.json {
        let providers = PROVIDERS
            .iter()
            .filter_map(|(key, _)| {
                let provider = creds.provider(key)?;
                if provider.api_key.is_empty() {
                    return None;
                }
                Some((
                    key.to_string(),
                    ProviderStatus {
                        configured: true,
                        mode: provider.connection.clone(),
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
        return util::emit_json(&status);
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
        if let Some(p) = creds.provider(key).filter(|p| !p.api_key.is_empty()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // Guards the field names the menubar app (AuthStatus.swift) decodes.
    #[test]
    fn json_shape_is_stable() {
        let status = AuthStatusJson {
            logged_in: true,
            profile: "default".into(),
            config_path: "/tmp/credentials.toml".into(),
            email: Some("a@b.co".into()),
            org_slug: Some("acme".into()),
            providers: BTreeMap::from([(
                "claude".to_string(),
                ProviderStatus {
                    configured: true,
                    mode: Some("plan".into()),
                },
            )]),
        };
        let v = serde_json::to_value(&status).unwrap();
        for key in ["logged_in", "profile", "config_path", "email", "org_slug", "providers"] {
            assert!(v.get(key).is_some(), "missing `{key}`");
        }
        assert_eq!(v["providers"]["claude"]["mode"], "plan");
    }
}
