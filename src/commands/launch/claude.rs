use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use console::style;

use super::util;
use crate::commands::util::plugins;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Launch through a local relay (MITM) proxy — same as `edgee relay claude`.
    #[arg(long)]
    pub relay: bool,

    /// Extra args passed through to the claude CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

const EDGEE_ALLOWED_TOOLS: &str = "mcp__edgee__setSessionName,mcp__edgee__addSessionPullRequest,mcp__edgee__addSessionCommit,mcp__edgee__setSessionGitRepo";

pub async fn run(opts: Options) -> Result<()> {
    if opts.relay {
        return crate::commands::relay::run_for_agent("claude").await;
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
            format!(
                "\nx-edgee-debug-pubkey: {}\nx-edgee-debug-salt: {}",
                headers.pubkey, headers.salt
            )
        })
        .unwrap_or_default();
    let mut cmd = std::process::Command::new(util::resolve_binary("claude"));

    // Set up the environment for the claude CLI to talk to Edgee's gateway instead of Anthropic's API.
    cmd
        .env("ANTHROPIC_BASE_URL", &gateway_url)
        .env(
            "ANTHROPIC_CUSTOM_HEADERS",
            format!(
                "x-edgee-api-key: {api_key}\nx-edgee-session-id: {session_id}{repo_header}{debug_log_header}"
            ),
        );

    // Set up the environment for the claude CLI to use 1M context window instead of the default 200k on Claude models.
    cmd.env("ANTHROPIC_DEFAULT_SONNET_MODEL", "claude-sonnet-5[1m]")
        .env("ANTHROPIC_DEFAULT_OPUS_MODEL", "claude-opus-5[1m]");

    // Set up the environment for Edgee session tracking and console API access.
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default());
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
                format!(
                    "{}/sessions/{slug}/{session_id}",
                    crate::config::console_base_url()
                )
            }
            _ => format!(
                "{}/sessions/{session_id}",
                crate::config::console_base_url()
            ),
        };
        let system_prompt =
            super::mcp::session_instructions(&session_id, repo_origin.as_deref(), &session_url);
        let system_prompt_path = write_system_prompt_file(&system_prompt)?;
        cmd.args(mcp_injection_args(&mcp_config_path, &system_prompt_path));
    }

    // Step 6: deliver the org's plugins. `--plugin-dir` loads a directory for
    // this session only and carries skills, subagents, hooks and MCP servers at
    // once, so nothing is ever written into the user's own `~/.claude`.
    //
    // Best-effort by construction: a failed fetch reuses whatever was
    // materialized last time, and a total failure delivers nothing. Neither
    // stops Claude from starting.
    let plugins = plugins::sync_for_target(&creds, plugins::Target::Claude).await;
    for dir in &plugins.plugin_dirs {
        cmd.arg("--plugin-dir").arg(dir);
    }
    plugins::report_launch(&plugins);

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
/// parser).
///
/// The system prompt is injected via `--append-system-prompt-file`, pointing at
/// a file on disk, rather than `--append-system-prompt <text>` inline. The
/// prompt text is multi-line, and on Windows `claude` commonly resolves to an
/// npm-installed `claude.cmd` batch shim rather than a native `.exe`. When the
/// spawned program is a `.bat`/`.cmd` file, `std::process::Command` rejects any
/// argument containing `\r`/`\n` outright — a CVE-2024-24576 ("BatBadBut")
/// mitigation — with the error "batch file arguments are invalid". Routing the
/// prompt through a file sidesteps that entirely, since the path itself is a
/// single line.
fn mcp_injection_args(config_path: &Path, system_prompt_path: &Path) -> Vec<OsString> {
    // Built as OsString rather than formatted: paths need not be UTF-8.
    let mut mcp_config = OsString::from("--mcp-config=");
    mcp_config.push(config_path);

    let mut system_prompt_file = OsString::from("--append-system-prompt-file=");
    system_prompt_file.push(system_prompt_path);

    vec![
        mcp_config,
        system_prompt_file,
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

/// Writes the Edgee session system prompt to a file in the Edgee config
/// directory, for use with `--append-system-prompt-file`. Returns the path to
/// the written file.
fn write_system_prompt_file(system_prompt: &str) -> Result<std::path::PathBuf> {
    let dir = crate::config::config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("system-prompt.txt");
    std::fs::write(&path, system_prompt)?;
    Ok(path)
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
        let injected = mcp_injection_args(
            Path::new("/tmp/mcp.json"),
            Path::new("/tmp/system-prompt.txt"),
        );

        assert!(injected.contains(&OsString::from("--mcp-config=/tmp/mcp.json")));
        assert!(injected.contains(&OsString::from(
            "--append-system-prompt-file=/tmp/system-prompt.txt"
        )));
        assert!(injected
            .iter()
            .any(|a| a.to_string_lossy() == format!("--allowedTools={EDGEE_ALLOWED_TOOLS}")));
        assert!(
            !injected.iter().any(|a| a == "--mcp-config"
                || a == "--allowedTools"
                || a == "--append-system-prompt-file"),
            "variadic flags must not be passed as a space-separated pair: {injected:?}"
        );
    }

    // Regression test: the system prompt used to be injected inline via
    // `--append-system-prompt <multi-line text>`. On Windows, when `claude`
    // resolves to an npm `claude.cmd` shim instead of a native `.exe`,
    // std::process::Command refuses any argument containing `\r`/`\n` for a
    // batch-file program with "batch file arguments are invalid". Passing the
    // prompt via a file path (which is always a single line) avoids that class
    // of bug entirely, regardless of what the prompt text contains.
    #[test]
    fn no_injected_arg_contains_a_newline() {
        let injected = mcp_injection_args(
            Path::new("/tmp/mcp.json"),
            Path::new("/tmp/system-prompt.txt"),
        );

        for arg in &injected {
            let text = arg.to_string_lossy();
            assert!(
                !text.contains('\n') && !text.contains('\r'),
                "arg must not contain a newline: {text:?}"
            );
        }
    }
}
