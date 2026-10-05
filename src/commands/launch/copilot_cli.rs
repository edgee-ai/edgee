use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::commands::relay::AgentExtras;
use crate::commands::util::plugins;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the copilot CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options, reroute: &super::reroute::Reroute) -> Result<()> {
    crate::commands::relay::run_for_agent_with_args("copilot-cli", &opts.args, reroute).await
}

/// Server name Copilot shows the Edgee tools under, and the `--allow-tool`
/// pattern that pre-approves all of them. Not plain `edgee`: Copilot keeps MCP
/// servers in one flat namespace and `--additional-mcp-config` silently shadows
/// a plugin server of the same name, and org plugins named `edgee` ship one.
const MCP_SERVER_NAME: &str = "edgee-session";

/// Session files the injected args point at, alive until [`CopilotSession::finish`].
pub(crate) struct CopilotSession {
    /// Holds the MCP config (it carries the user token) and the session
    /// instructions.
    session_dir: Option<tempfile::TempDir>,
}

impl CopilotSession {
    pub(crate) async fn finish(self, session_id: &str) {
        drop(self.session_dir);
        if let Ok(creds) = crate::config::read() {
            super::print_session_stats(&creds, session_id, "Copilot").await;
        }
    }
}

/// Builds the Copilot-specific part of a relayed launch. Runs after the relay
/// created `session_id`, since the MCP instructions and the console link need
/// it. `interactive` is false under `edgee relay --non-interactive`, which must
/// never prompt.
pub(crate) async fn prepare(
    session_id: &str,
    interactive: bool,
) -> Result<(AgentExtras, CopilotSession)> {
    let mut creds = crate::config::read()?;
    let mut extras = AgentExtras::default();

    // The statusline renderer reads this to reach the active profile's API.
    extras
        .env
        .push(("EDGEE_CONSOLE_API_URL", crate::config::console_api_base_url().into()));

    crate::commands::statusline::copilot::install::ensure_installed_on_launch().await;

    let org = super::fetch_active_org(&creds).await;
    let mcp_disabled = super::mcp_injection_disabled_with_org(org.as_ref());
    if !mcp_disabled && interactive {
        crate::commands::auth::login::ensure_mcp_preference().await?;
        creds = crate::config::read()?;
    }
    let wants_mcp = creds.enable_mcp.unwrap_or(false);
    if mcp_disabled && wants_mcp {
        super::mcp::print_injection_skipped();
    }

    let mut session_dir = None;
    if wants_mcp && !mcp_disabled {
        let dir = tempfile::Builder::new()
            .prefix("edgee-copilot-")
            .tempdir()
            .context("creating the Copilot session directory")?;
        let token = creds.user_token.as_deref().unwrap_or("");
        let mcp_config = write_mcp_config(dir.path(), token)?;
        let instructions = super::mcp::session_instructions(
            session_id,
            crate::git::detect_origin().as_deref(),
            &super::mcp::session_url(&creds, session_id),
        );
        let instructions_dir = write_instructions(dir.path(), &instructions)?;

        extras.args.extend(mcp_injection_args(&mcp_config));
        extras.env.push((
            "COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
            custom_instructions_dirs(
                std::env::var_os("COPILOT_CUSTOM_INSTRUCTIONS_DIRS"),
                &instructions_dir,
            ),
        ));
        session_dir = Some(dir);
    }

    // Org plugins. `--plugin-dir` loads a bundle for this session only, so
    // nothing lands in the user's `~/.copilot`.
    let report = plugins::sync_for_target(&creds, plugins::Target::CopilotCli).await;
    extras.args.extend(plugin_dir_args(&report.plugin_dirs));
    plugins::report_launch(&report);

    Ok((extras, CopilotSession { session_dir }))
}

/// `--allow-tool` is variadic in Copilot CLI, so injected flags use the `=` form
/// (see the launch argv rule in `CLAUDE.md`). The config goes by `@file`: inline
/// JSON would put the bearer token in argv.
fn mcp_injection_args(config_path: &Path) -> Vec<OsString> {
    let mut config = OsString::from("--additional-mcp-config=@");
    config.push(config_path);
    vec![
        config,
        OsString::from(format!("--allow-tool={MCP_SERVER_NAME}")),
    ]
}

fn plugin_dir_args(dirs: &[PathBuf]) -> Vec<OsString> {
    dirs.iter()
        .map(|dir| {
            let mut arg = OsString::from("--plugin-dir=");
            arg.push(dir);
            arg
        })
        .collect()
}

fn mcp_config(token: &str) -> serde_json::Value {
    let mut server = super::mcp::edgee_http_server(token);
    server["tools"] = serde_json::json!(["*"]);
    serde_json::json!({ "mcpServers": { MCP_SERVER_NAME: server } })
}

fn write_mcp_config(dir: &Path, token: &str) -> Result<PathBuf> {
    let path = dir.join("mcp.json");
    std::fs::write(&path, serde_json::to_string_pretty(&mcp_config(token))?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Copilot reads `.github/instructions/*.instructions.md` from each directory in
/// `COPILOT_CUSTOM_INSTRUCTIONS_DIRS` (verified with `copilot instruction list`,
/// 1.0.90); a bare `copilot-instructions.md` there is ignored. Returns the
/// directory to list in that variable.
fn write_instructions(dir: &Path, instructions: &str) -> Result<PathBuf> {
    let root = dir.join("instructions");
    let file_dir = root.join(".github").join("instructions");
    std::fs::create_dir_all(&file_dir)
        .with_context(|| format!("creating {}", file_dir.display()))?;
    let path = file_dir.join("edgee-session.instructions.md");
    std::fs::write(&path, format!("---\napplyTo: \"**\"\n---\n\n{instructions}\n"))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(root)
}

/// Appends ours to a value the user already exported rather than replacing it.
fn custom_instructions_dirs(existing: Option<OsString>, ours: &Path) -> OsString {
    match existing.filter(|v| !v.is_empty()) {
        Some(mut dirs) => {
            dirs.push(",");
            dirs.push(ours);
            dirs
        }
        None => ours.as_os_str().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_flags_use_equals_form_and_keep_the_token_out_of_argv() {
        let args = mcp_injection_args(Path::new("/tmp/edgee-copilot-x/mcp.json"));
        assert_eq!(
            args,
            vec![
                OsString::from("--additional-mcp-config=@/tmp/edgee-copilot-x/mcp.json"),
                OsString::from("--allow-tool=edgee-session"),
            ]
        );
    }

    #[test]
    fn plugin_dirs_use_equals_form() {
        assert_eq!(
            plugin_dir_args(&[PathBuf::from("/p/a"), PathBuf::from("/p/b")]),
            vec![OsString::from("--plugin-dir=/p/a"), OsString::from("--plugin-dir=/p/b")]
        );
    }

    #[test]
    fn mcp_config_matches_copilot_schema() {
        let config = mcp_config("tok");
        let server = &config["mcpServers"]["edgee-session"];
        assert_eq!(server["type"], "http");
        assert_eq!(server["headers"]["Authorization"], "Bearer tok");
        assert_eq!(server["tools"], serde_json::json!(["*"]));
        assert!(server["url"].as_str().is_some_and(|u| !u.is_empty()));
    }

    #[test]
    fn instructions_land_where_copilot_looks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_instructions(tmp.path(), "call setSessionName").unwrap();
        let file = root.join(".github/instructions/edgee-session.instructions.md");
        let body = std::fs::read_to_string(file).unwrap();
        assert!(body.starts_with("---\napplyTo: \"**\"\n---\n"));
        assert!(body.contains("call setSessionName"));
    }

    #[test]
    fn custom_instructions_dirs_appends_to_the_users_value() {
        let ours = Path::new("/tmp/ours");
        assert_eq!(custom_instructions_dirs(None, ours), OsString::from("/tmp/ours"));
        assert_eq!(
            custom_instructions_dirs(Some(OsString::new()), ours),
            OsString::from("/tmp/ours")
        );
        assert_eq!(
            custom_instructions_dirs(Some(OsString::from("/a,/b")), ours),
            OsString::from("/a,/b,/tmp/ours")
        );
    }
}
