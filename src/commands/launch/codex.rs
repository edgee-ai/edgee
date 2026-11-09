use anyhow::Result;

use super::util;
use crate::commands::util::plugins;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the codex CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Bare MCP tool names, matching how Codex's `enabled_tools` addresses tools on
/// a named server — unlike Claude's `mcp__edgee__<tool>` allowedTools entries,
/// which are Claude's own namespacing convention, not the underlying tool name.
const EDGEE_ENABLED_TOOLS: &[&str] = &[
    "setSessionName",
    "addSessionPullRequest",
    "addSessionCommit",
    "setSessionGitRepo",
];

/// `-c mcp_servers.edgee.*` overrides for Edgee's own session-tracking MCP
/// server — the same server claude.rs points `--mcp-config` at, delivered here
/// as TOML overrides instead of a JSON file, the same way `codex_mcp_args`
/// delivers plugin MCP servers.
fn edgee_mcp_args(token: &str, launch_id: &str) -> Vec<String> {
    let mut args = vec![format!(
        "mcp_servers.edgee.url={}",
        plugins::config::toml_string(&crate::config::mcp_base_url())
    )];

    let headers = super::mcp::edgee_http_headers(token, launch_id);
    args.push(format!(
        "mcp_servers.edgee.http_headers={}",
        plugins::config::toml_table(&headers)
    ));

    let tools = EDGEE_ENABLED_TOOLS
        .iter()
        .map(|t| plugins::config::toml_string(t))
        .collect::<Vec<_>>()
        .join(", ");
    args.push(format!("mcp_servers.edgee.enabled_tools=[{tools}]"));

    args
}

/// Add session guidance without replacing Codex's built-in model instructions.
/// Like our provider overrides, this replaces the corresponding config value;
/// an explicit user `-c developer_instructions=...` passed later takes precedence.
fn developer_instructions_arg(session_id: &str, repo: Option<&str>, session_url: &str) -> String {
    format!(
        "developer_instructions={}",
        plugins::config::toml_string(&super::mcp::session_instructions(session_id, repo, session_url))
    )
}

pub async fn run(opts: Options, reroute: &super::pause::Controls) -> Result<()> {
    if reroute.pause.pause_optimizations {
        reject_pause_transport_overrides(&opts.args)?;
    }
    ensure_saved_thread_provider()?;
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for Codex. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("codex")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("codex").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan" for codex)
    if creds
        .codex
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.codex.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    let org = super::fetch_active_org(&creds).await;
    let mcp_disabled = super::mcp_injection_disabled_with_org(org.as_ref());
    if !mcp_disabled {
        crate::commands::auth::login::ensure_mcp_preference().await?;
        creds = crate::config::read()?;
    }

    // Step 3: launch codex with the correct env vars
    let codex = creds.codex.as_ref().unwrap();
    let api_key = &codex.api_key;
    let gateway_url = super::gateway_base_url_with_org(org.as_ref());
    let (session_id, requirement) = reroute.create_session(&creds, "codex", &gateway_url).await?;
    let pause_headers = super::pause::toml_headers(requirement.as_ref());


    let repo_origin = crate::git::detect_origin();
    let repo_entry = repo_origin.as_ref()
        .map(|url| format!(",\"x-edgee-repo\"=\"{url}\""))
        .unwrap_or_default();
    let debug_log_entry = util::resolve_debug_log_keypair()?
        .map(|keypair| {
            let headers = keypair.header_values();
            format!(",\"x-edgee-debug-pubkey\"=\"{}\",\"x-edgee-debug-salt\"=\"{}\"", headers.pubkey, headers.salt)
        })
        .unwrap_or_default();
    let base_url = format!("{gateway_url}/v1");
    let codex_bin = util::resolve_binary("codex");
    let mut cmd = std::process::Command::new(&codex_bin);
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default());
    cmd.args([
        "-c", "model_provider=\"edgee-cli\"",
        "-c", "model_providers.edgee-cli.name=\"EDGEE\"",
        "-c", &format!("model_providers.edgee-cli.base_url=\"{base_url}\""),
        "-c", &format!("model_providers.edgee-cli.http_headers={{\"x-edgee-api-key\"=\"{api_key}\",\"x-edgee-session-id\"=\"{session_id}\"{repo_entry}{debug_log_entry}{pause_headers}}}"),
        "-c", "model_providers.edgee-cli.wire_api=\"responses\"",
        // Codex only attaches the user's ChatGPT `Authorization: Bearer` token to a
        // custom provider when this is set. It used to default to on; codex 0.149
        // flipped it, and every plan-connection request started 401ing gateway-side
        // because no upstream credential arrived. Harmless when the user isn't
        // logged in to codex — the header is simply omitted.
        "-c", "model_providers.edgee-cli.requires_openai_auth=true",
    ]);
    // Edgee model catalog for Codex's picker and model metadata; see `codex_catalog`.
    for arg in super::codex_catalog::cli_args(&codex_bin, "edgee-cli", &base_url, api_key).await {
        cmd.args(["-c", &arg]);
    }
    // Org plugins. Codex exposes no way to add a skills directory — only to
    // relocate the whole config root via CODEX_HOME — so we build a symlink
    // mirror of it and add ours there. Symlinks mean the user's auth.json,
    // history and databases are referenced, never copied, and their real
    // ~/.codex is never written to. MCP servers ride `-c` overrides, the same
    // mechanism already used above for the provider.
    let plugin_report = plugins::sync_for_target(&creds, plugins::Target::Codex).await;
    for arg in plugins::config::codex_mcp_args(&plugin_report.plugins) {
        cmd.args(["-c", &arg]);
    }

    // Edgee's own session-tracking MCP server, gated the same way claude.rs
    // gates injection (org policy, then the member's own preference).
    let use_mcp = creds.enable_mcp.unwrap_or(false) && !mcp_disabled;
    if use_mcp {
        let token = creds.user_token.as_deref().unwrap_or("");
        for arg in edgee_mcp_args(token, &session_id) {
            cmd.args(["-c", &arg]);
        }
        let session_url = super::mcp::session_url(&creds, &session_id)?;
        cmd.args(["-c", &developer_instructions_arg(&session_id, repo_origin.as_deref(), &session_url)]);
    }
    if let Some(skills_root) = plugin_report.skills_root.as_ref() {
        if let Some(mirror) = plugins::codex_home_mirror() {
            let source = plugins::codex_home_source();
            // Delivering nothing beats delivering into a half-built mirror, so a
            // failure here simply leaves CODEX_HOME alone.
            if plugins::mirror::build(&source, &mirror, &["skills"]).is_ok()
                && plugins::mirror::link_children(skills_root, &mirror.join("skills")).is_ok()
            {
                cmd.env("CODEX_HOME", &mirror);
            }
        }
    }
    plugins::report_launch(&plugin_report);

    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Codex CLI is not installed. Install it from https://developers.openai.com/codex/cli"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "Codex").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

/// Codex applies later -c overrides last. An explicit pause cannot let a user's
/// provider override erase the required headers or select an unregistered key.
fn reject_pause_transport_overrides(args: &[String]) -> Result<()> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        anyhow::ensure!(
            !matches!(arg.as_str(), "--oss" | "--local-provider")
                && !arg.starts_with("--oss=")
                && !arg.starts_with("--local-provider="),
            "Codex local-provider flags bypass Edgee. Omit them when using --pause-optimizations."
        );
        let config = if arg == "-c" || arg == "--config" {
            args.next().map(String::as_str)
        } else {
            arg.strip_prefix("--config=").or_else(|| arg.strip_prefix("-c").filter(|value| !value.is_empty()))
        };
        if let Some(config) = config {
            let key = config.split_once('=').map(|(key, _)| key).unwrap_or(config);
            let path: toml::Table = format!("{key}=true").parse()
                .map_err(|_| anyhow::anyhow!("Cannot verify Codex config override for a paused launch: {key}"))?;
            anyhow::ensure!(!["model_provider", "model_providers", "profiles", "profile"].iter().any(|key| path.contains_key(*key)),
                "Codex provider/profile overrides can remove required pause headers. Omit this override when using --pause-optimizations, or use organization pause.");
        }
    }
    Ok(())
}

/// Also run when removing the alias, to repair threads from older Edgee versions.
pub(crate) fn ensure_saved_thread_provider() -> Result<()> {
    super::codex_config::ensure_fallback(&plugins::codex_home_source().join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_codex_cannot_override_its_registered_provider_or_requirement_headers() {
        for args in [
            vec!["-c", "model_providers.edgee-cli.http_headers={}"],
            vec!["--config=model_provider=\"other\""],
            vec!["-cmodel_providers={}"],
            vec!["-c", "'model_provider' = \"other\""],
            vec!["--oss"],
            vec!["--local-provider=ollama"],
        ] {
            let args = args.into_iter().map(String::from).collect::<Vec<_>>();
            assert!(reject_pause_transport_overrides(&args).is_err());
        }
        let args = ["-c", "model_reasoning_effort=high", "--", "-c", "literal prompt"]
            .into_iter().map(String::from).collect::<Vec<_>>();
        assert!(reject_pause_transport_overrides(&args).is_ok());
    }

    #[test]
    fn developer_instructions_round_trip_session_metadata() {
        let session_id = "session-123";
        let repo = "https://example.com/a/\"repo\"";
        let url = "https://console.example/~/org/sessions/session-123";
        let arg = developer_instructions_arg(session_id, Some(repo), url);
        let parsed: toml::Table = arg.parse().unwrap();
        let instructions = parsed["developer_instructions"].as_str().unwrap();
        assert!(instructions.contains(session_id));
        assert!(instructions.contains(repo));
        assert!(instructions.contains(url));
        assert!(instructions.contains("setSessionName"));
        assert!(instructions.contains("addSessionPullRequest"));
        assert!(instructions.contains("setSessionGitRepo"));
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn developer_instructions_omit_repo_tool_without_origin() {
        let arg = developer_instructions_arg("session-123", None, "https://example.com/session");
        let parsed: toml::Table = arg.parse().unwrap();
        assert!(!parsed["developer_instructions"].as_str().unwrap().contains("setSessionGitRepo"));
    }

    #[test]
    fn edgee_mcp_args_are_well_formed_toml_overrides() {
        let args = edgee_mcp_args("tok_123", "launch-current");

        assert!(args
            .iter()
            .any(|a| a.starts_with("mcp_servers.edgee.url=") && a.contains(&crate::config::mcp_base_url())));
        let headers = args.iter().find(|arg| arg.starts_with("mcp_servers.edgee.http_headers=")).unwrap();
        let parsed: toml::Table = headers.parse().unwrap();
        assert_eq!(parsed["mcp_servers"]["edgee"]["http_headers"]["Authorization"].as_str(), Some("Bearer tok_123"));
        assert_eq!(parsed["mcp_servers"]["edgee"]["http_headers"]["X-Edgee-Launch-Id"].as_str(), Some("launch-current"));
        assert!(args.contains(&format!(
            "mcp_servers.edgee.enabled_tools=[{}]",
            EDGEE_ENABLED_TOOLS
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    /// A token with a quote or newline must not break out of the TOML string —
    /// same hazard `codex_toml_strings_escape_control_characters` guards for
    /// plugin MCP servers.
    #[test]
    fn edgee_mcp_args_escape_control_characters_in_the_token() {
        let args = edgee_mcp_args("say \"hi\"\nthere", "launch-current");
        let headers = args
            .iter()
            .find(|a| a.starts_with("mcp_servers.edgee.http_headers="))
            .unwrap();

        assert!(headers.contains(r#"\""#));
        assert!(headers.contains(r"\n"));
        assert!(!headers.contains('\n'));
    }
}
