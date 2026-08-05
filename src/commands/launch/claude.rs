use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use console::style;

use super::util;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the claude CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

const EDGEE_ALLOWED_TOOLS: &str = "mcp__edgee__setSessionName,mcp__edgee__addSessionPullRequest,mcp__edgee__addSessionCommit,mcp__edgee__setSessionGitRepo";

pub async fn run(opts: Options) -> Result<()> {
    // `--relay` moved ahead of the target so the passthrough stays clean. Claude
    // Code has no such flag, so the old spelling would reach it and fail with a
    // bare "unknown option" — name the actual fix instead.
    if opts.args.first().is_some_and(|a| a == "--relay") {
        anyhow::bail!("`--relay` now goes before the target: `edgee launch --relay claude`");
    }

    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for Claude. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let key_status = crate::commands::auth::login::ensure_valid_provider_key("claude").await?;
    if key_status.created {
        crate::commands::auth::login::ensure_onboarded("claude").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan")
    if creds
        .claude
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.claude.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 3b: ensure MCP preference is set — unless injection is off for this
    // launch, in which case the local answer would be moot. Fetched once here
    // and reused below for the gateway URL.
    let org = super::fetch_active_org(&creds).await;
    let mcp_disabled = super::mcp_injection_disabled_with_org(org.as_ref());
    if !mcp_disabled {
        crate::commands::auth::login::ensure_mcp_preference().await?;
        creds = crate::config::read()?;
    }

    // Step 4: launch claude with the correct env vars
    let claude = creds.claude.as_ref().unwrap();
    let api_key = &claude.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    let repo_origin = crate::git::detect_origin();
    let repo_header = repo_origin
        .as_ref()
        .map(|url| format!("\nx-edgee-repo: {url}"))
        .unwrap_or_default();

    // First-run: install the persistent user-level statusline integration
    // exactly once (honors the disable marker).
    util::ensure_first_run_installed().await;

    util::spawn_cli_version_report(&creds, &session_id);

    let gateway_url = super::gateway_base_url_with_org(org.as_ref());
    let debug_log_header = util::resolve_debug_log_keypair()?
        .map(|keypair| {
            let headers = keypair.header_values();
            format!("\nx-edgee-debug-pubkey: {}\nx-edgee-debug-salt: {}", headers.pubkey, headers.salt)
        })
        .unwrap_or_default();
    let mut cmd = std::process::Command::new(util::resolve_binary("claude"));
    cmd.env("ANTHROPIC_BASE_URL", &gateway_url);
    cmd.env(
        "ANTHROPIC_CUSTOM_HEADERS",
        format!(
            "x-edgee-api-key: {api_key}\nx-edgee-session-id: {session_id}{repo_header}{debug_log_header}"
        ),
    );
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env(
        "EDGEE_CONSOLE_API_URL",
        crate::config::console_api_base_url(),
    );

    // Force-enable Claude Code's client-side "MCP Tool Search" when the key has
    // tool_surface_reduction enabled, unless the user has explicitly set it
    // themselves. Reuses the compression settings already fetched by
    // `ensure_valid_provider_key` above instead of a second `get_key_by_id` call.
    let tool_surface_reduction_enabled = key_status
        .compression
        .map(|c| c.tool_surface_reduction)
        .unwrap_or(false);
    if tool_surface_reduction_enabled && std::env::var_os("ENABLE_TOOL_SEARCH").is_none() {
        cmd.env("ENABLE_TOOL_SEARCH", "true");
    }

    // Step 5: conditionally set up MCP integration. Injection being off is a
    // hard override — a member who opted in locally still gets none, unless
    // they set the env var (see `mcp_injection_disabled_with_org`).
    let wants_mcp = creds.enable_mcp.unwrap_or(false);
    if mcp_disabled && wants_mcp {
        // Without this the integration would just silently vanish, which reads
        // as a bug rather than a deliberate setting. Name the actual source, so
        // a forgotten export doesn't look like an org decision.
        let reason = if crate::config::mcp_injection_disabled_env_override() == Some(true) {
            "EDGEE_MCP_INJECTION_DISABLED is set"
        } else {
            "Edgee MCP is turned off for your organization"
        };
        println!("{}", style(format!("  {reason} — skipping.")).dim());
    }
    let use_mcp = wants_mcp && !mcp_disabled;
    if use_mcp {
        let mcp_config_path = write_mcp_config(&creds)?;
        let session_url = match creds.org_slug.as_deref() {
            Some(slug) if !slug.is_empty() => {
                format!("{}/sessions/{slug}/{session_id}", crate::config::console_base_url())
            }
            _ => format!("{}/sessions/{session_id}", crate::config::console_base_url()),
        };
        cmd.args(mcp_injection_args(
            &mcp_config_path,
            &system_prompt(&session_id, repo_origin.as_deref(), &session_url),
        ));
    }

    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Claude Code is not installed. Install it from https://code.claude.com/docs/en/quickstart"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "Claude").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

/// Flags that wire the Edgee MCP server into a session.
///
/// `--mcp-config <configs...>` and `--allowedTools <tools...>` are **variadic**
/// in Claude Code, so they are passed as `--flag=value`. Separated by a space,
/// commander keeps consuming every following non-flag arg as another value and
/// eats whatever the user appended — their prompt (`claude "fix this"` starts an
/// empty session) or a subcommand (`claude mcp add --transport http …` fails with
/// "unknown option '--transport'", because `--transport` then lands on the root
/// parser). `--append-system-prompt` takes a single value and is safe as a pair.
fn mcp_injection_args(config_path: &Path, system_prompt: &str) -> Vec<OsString> {
    // Built as OsString rather than formatted: config_path need not be UTF-8.
    let mut mcp_config = OsString::from("--mcp-config=");
    mcp_config.push(config_path);

    vec![
        mcp_config,
        OsString::from("--append-system-prompt"),
        OsString::from(system_prompt),
        OsString::from(format!("--allowedTools={EDGEE_ALLOWED_TOOLS}")),
    ]
}

/// Writes an MCP config file to the Edgee config directory with the user's auth token.
/// Returns the path to the written file.
fn write_mcp_config(creds: &crate::config::Credentials) -> Result<std::path::PathBuf> {
    let token = creds.user_token.as_deref().unwrap_or("");
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "edgee": {
                "type": "http",
                "url": crate::config::mcp_base_url(),
                "headers": {
                    "Authorization": format!("Bearer {token}")
                }
            }
        }
    });

    let dir = crate::config::config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("mcp.json");
    std::fs::write(&path, serde_json::to_string_pretty(&mcp_config)?)?;
    Ok(path)
}

fn system_prompt(session_id: &str, repo: Option<&str>, session_url: &str) -> String {
    let mut prompt = format!(
        r#"You are running inside the Edgee CLI and have access to the Edgee MCP server for tracking session metadata.

Your Edgee session ID is: {session_id}
Your Edgee public session page is: {session_url}

You MUST use the following Edgee MCP tools during this session:

1. `setSessionName` — call this immediately after the user's first message with a short descriptive name (3-6 words) summarizing what the user is asking for. Arguments:
   - sessionId: "{session_id}"
   - name: the descriptive name.
   If at any later point during the session you come up with a clearly better name (e.g., the task's real scope becomes obvious only after exploring the code, or the user pivots the request), call `setSessionName` again with the improved name. Prefer calling it once, but do not hesitate to update when a materially better name emerges.

2. `addSessionPullRequest` — call this EVERY TIME you create OR edit a pull request (e.g., via `gh pr create`, `gh pr edit`, or any other tool). Immediately after the PR is created or modified, call this tool with:
   - sessionId: "{session_id}"
   - pullRequest: the full PR URL.
   This is required for every PR you touch during this session, with no exceptions. Always call it on edits too — the PR may not yet be associated with this session, and the API handles duplicates safely, so redundant calls are harmless."#
    );

    if let Some(repo) = repo {
        prompt.push_str(&format!(
            "\n\n3. `setSessionGitRepo` — call this EXACTLY ONCE at the start of the session, together with (or right after) `setSessionName`. Arguments:\n   - sessionId: \"{session_id}\"\n   - repo: \"{repo}\"\n   Do not call this tool again during the session."
        ));
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    // Claude Code's `--mcp-config <configs...>` and `--allowedTools <tools...>`
    // are variadic: passed as `--flag value`, commander swallows every following
    // non-flag arg. Since the user's own args are appended after these, a space
    // separator eats their prompt (`claude "fix this"` → empty session) or their
    // subcommand (`claude mcp add --transport http …` → the subcommand is eaten
    // and `--transport` errors on the root parser). `=` stops the swallowing.
    #[test]
    fn variadic_injected_flags_use_equals_form() {
        let injected = mcp_injection_args(Path::new("/tmp/mcp.json"), "sys prompt");

        assert!(injected.contains(&OsString::from("--mcp-config=/tmp/mcp.json")));
        assert!(injected
            .iter()
            .any(|a| a.to_string_lossy() == format!("--allowedTools={EDGEE_ALLOWED_TOOLS}")));
        assert!(
            !injected
                .iter()
                .any(|a| a == "--mcp-config" || a == "--allowedTools"),
            "variadic flags must not be passed as a space-separated pair: {injected:?}"
        );
    }

    // Single-valued, so the pair form is safe — and required, since the prompt
    // is multi-line text we should not have to escape into a `--flag=value`.
    #[test]
    fn append_system_prompt_stays_a_separate_value_arg() {
        let injected = mcp_injection_args(Path::new("/tmp/mcp.json"), "sys prompt");
        let at = injected
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("flag present");

        assert_eq!(injected[at + 1], OsString::from("sys prompt"));
    }
}
