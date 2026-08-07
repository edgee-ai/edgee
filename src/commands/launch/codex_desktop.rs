//! `edgee launch codex-desktop` — the ChatGPT desktop app through Edgee.
//!
//! No relay needed: the app's backend is a bundled `codex app-server` reading
//! `$CODEX_HOME/config.toml`, which honors the same provider settings
//! [`super::codex`] passes the CLI as `-c` overrides.
//!
//! Lifecycle: patch `config.toml`, spawn the app detached, wait out
//! [`CONFIG_HANDOFF_GRACE`], revert, exit. The app caches the config at startup, so
//! the patch need not outlive the handoff — which keeps the `codex` CLI unaffected
//! and leaves no window where a crash could strand the patch.
//!
//! Why not something cleaner (all verified against codex 0.147):
//! `--profile` would need argv we can't set (no `CODEX_PROFILE` env var, and
//! `profile = "…"` in config is a fatal error); a private `CODEX_HOME` needs its own
//! `auth.json`, and the ChatGPT refresh token is single-use/rotating so two copies
//! invalidate each other (symlinks don't help — codex atomically replaces the file);
//! `CODEX_ACCESS_TOKEN` is an agent-identity slot, so env auth would bill via API
//! key instead of the user's plan. `auth.json` is never touched.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use console::style;

use super::util;

/// Same provider id as the CLI target, so sessions look identical to the gateway.
const PROVIDER_ID: &str = "edgee-cli";

/// Bracket everything we insert, so it can be removed without touching the rest.
const MANAGED_BEGIN: &str = "# >>> edgee managed block — removed when the app exits";
const MANAGED_END: &str = "# <<< edgee managed block";

#[derive(Debug, clap::Parser)]
pub struct Options {}

pub async fn run(_opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Its own `codex_desktop` provider key, so console and stats can tell the
    // desktop app apart from the CLI (same split as `claude_desktop`).
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }
    crate::commands::auth::login::ensure_org_selected().await?;
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("codex_desktop")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("codex_desktop").await?;
    }
    creds = crate::config::read()?;

    let api_key = creds
        .codex_desktop
        .as_ref()
        .map(|c| c.api_key.clone())
        .ok_or_else(|| {
            anyhow::anyhow!("no Edgee API key for 'codex_desktop'; run `edgee auth login`")
        })?;
    let session_id = uuid::Uuid::new_v4().to_string();

    util::ensure_first_run_installed().await;
    util::spawn_cli_version_report(&creds, &session_id);

    let base_url = format!("{}/v1", super::resolve_gateway_base_url(&creds).await);
    let debug_headers = util::resolve_debug_log_keypair()?.map(|k| k.header_values());
    let block = render_provider_block(
        &base_url,
        &api_key,
        &session_id,
        crate::git::detect_origin().as_deref(),
        debug_headers.as_ref(),
    );

    let binary = app_binary()?;
    let config_path = codex_config_path()?;
    let backup = backup_path(&config_path);

    // A leftover backup means a previous run died before reverting. Undo it first, or
    // we'd snapshot our own patched config and "restore" to it forever.
    if backup.exists() {
        restore_config(&config_path, &backup)?;
        eprintln!(
            "{}",
            style("Recovered your codex config from an interrupted previous run.").dim()
        );
    }

    apply_provider_block(&config_path, &backup, &block)?;

    print_launch_hint(&base_url, &session_id);

    let mut child = match spawn_app(&binary) {
        Ok(child) => child,
        Err(e) => {
            // Never leave the config patched if the app didn't start.
            let _ = restore_config(&config_path, &backup);
            return Err(e);
        }
    };

    let quick_exit = wait_out_config_handoff(&mut child).await;
    restore_config(&config_path, &backup)?;

    // Single-instance lock: if an instance was already running, ours handed off to it
    // and exited — and that one loaded the unpatched config, so it bypasses Edgee.
    if quick_exit {
        eprintln!(
            "{}",
            style(
                "The ChatGPT desktop app was already running, so this launch handed off to \
                 the existing instance — which is NOT going through Edgee. Quit it \
                 completely (Cmd-Q), then re-run `edgee launch codex-desktop`."
            )
            .yellow()
        );
        std::process::exit(1);
    }

    print_handoff_complete(&creds, &session_id);

    Ok(())
}

/// Window for the app's codex to start and read the patched config. Measured need is
/// ~2s; if a cold start ever exceeded this the app just talks to OpenAI directly.
const CONFIG_HANDOFF_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Exit this fast means the app never started — it hit the single-instance lock.
const HANDOFF_EXIT_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);

/// Hold the patch for [`CONFIG_HANDOFF_GRACE`]; true if the app handed off instead of
/// starting. Polls so that case is caught without waiting out the full grace.
async fn wait_out_config_handoff(child: &mut tokio::process::Child) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < CONFIG_HANDOFF_GRACE {
        if let Ok(Some(status)) = child.try_wait() {
            return status.success() && started.elapsed() < HANDOFF_EXIT_WINDOW;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    false
}

/// Escape a value for a double-quoted TOML basic string.
fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Kept separate because the two fragments go to different places — see
/// [`patch_config_text`].
struct ProviderBlock {
    selector: String,
    tables: String,
}

/// Mirrors the `-c` overrides in [`super::codex`].
fn render_provider_block(
    base_url: &str,
    api_key: &str,
    session_id: &str,
    repo: Option<&str>,
    debug_headers: Option<&crate::crypto::DebugLogHeaderValues>,
) -> ProviderBlock {
    let mut tables = format!(
        "[model_providers.{PROVIDER_ID}]\n\
         name = \"EDGEE\"\n\
         base_url = \"{}\"\n\
         wire_api = \"responses\"\n\n\
         [model_providers.{PROVIDER_ID}.http_headers]\n\
         \"x-edgee-api-key\" = \"{}\"\n\
         \"x-edgee-session-id\" = \"{}\"\n",
        toml_escape(base_url),
        toml_escape(api_key),
        toml_escape(session_id),
    );
    if let Some(repo) = repo {
        tables.push_str(&format!("\"x-edgee-repo\" = \"{}\"\n", toml_escape(repo)));
    }
    if let Some(debug) = debug_headers {
        tables.push_str(&format!(
            "\"x-edgee-debug-pubkey\" = \"{}\"\n\"x-edgee-debug-salt\" = \"{}\"\n",
            toml_escape(&debug.pubkey),
            toml_escape(&debug.salt),
        ));
    }

    ProviderBlock {
        selector: format!(
            "{MANAGED_BEGIN}\nmodel_provider = \"{PROVIDER_ID}\"\n{MANAGED_END}\n"
        ),
        tables: format!("{MANAGED_BEGIN}\n{tables}{MANAGED_END}\n"),
    }
}

/// Remove any managed block, making both patch and restore idempotent.
fn strip_managed_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == MANAGED_BEGIN {
            inside = true;
            continue;
        }
        if trimmed == MANAGED_END {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Splice `block` in at the first table header: `model_provider` must precede it (a
/// top-level scalar after a table header joins that table) and our tables must follow
/// all the user's scalars (or `model = "…"` gets absorbed into our headers table).
/// Textual, not parse-and-reserialize, so comments and formatting survive.
fn patch_config_text(existing: &str, block: &ProviderBlock) -> String {
    let existing = strip_managed_blocks(existing);

    let first_table = existing
        .lines()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(usize::MAX);

    let mut head = String::new();
    let mut tail = String::new();
    for (i, line) in existing.lines().enumerate() {
        // Drop the user's own selector; the backup restores it.
        if i < first_table && line.trim_start().starts_with("model_provider") {
            continue;
        }
        if i < first_table {
            head.push_str(line);
            head.push('\n');
        } else {
            tail.push_str(line);
            tail.push('\n');
        }
    }

    let mut out = head;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&block.selector);
    out.push('\n');
    out.push_str(&block.tables);
    out.push('\n');
    out.push_str(&tail);
    out
}

/// Back up `config_path` and write the patched version.
fn apply_provider_block(config_path: &Path, backup: &Path, block: &ProviderBlock) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // A missing config is fine: an empty base is a valid starting point.
    let existing = std::fs::read_to_string(config_path).unwrap_or_default();

    std::fs::write(backup, &existing)
        .with_context(|| format!("writing config backup {}", backup.display()))?;

    let patched = patch_config_text(&existing, block);
    std::fs::write(config_path, patched)
        .with_context(|| format!("patching {}", config_path.display()))?;
    Ok(())
}

/// Strip our block from the config as it stands **now**, rather than rolling back to
/// the backup: codex rewrites this file at runtime (trust levels, plugin state), and a
/// rollback would discard the user's session. Backup is only a fallback. No-op when
/// there's nothing to undo.
fn restore_config(config_path: &Path, backup: &Path) -> Result<()> {
    if !backup.exists() {
        return Ok(());
    }
    let backup_body = std::fs::read_to_string(backup)
        .with_context(|| format!("reading config backup {}", backup.display()))?;
    let current = std::fs::read_to_string(config_path).unwrap_or_default();

    if current.contains(MANAGED_BEGIN) {
        let stripped = strip_managed_blocks(&current);
        // An empty backup means the file didn't exist before us.
        if backup_body.is_empty() && stripped.trim().is_empty() {
            let _ = std::fs::remove_file(config_path);
        } else {
            std::fs::write(config_path, stripped)
                .with_context(|| format!("restoring {}", config_path.display()))?;
        }
    } else if backup_body.is_empty() {
        let _ = std::fs::remove_file(config_path);
    } else {
        // Markers gone (they're comments — codex may have reserialized the file), so we
        // can't tell our lines from the user's. Losing runtime writes beats leaving a
        // live key behind.
        std::fs::write(config_path, &backup_body)
            .with_context(|| format!("restoring {}", config_path.display()))?;
    }

    let _ = std::fs::remove_file(backup);
    Ok(())
}

/// `$CODEX_HOME/config.toml`, honoring the override codex itself reads.
fn codex_config_path() -> Result<PathBuf> {
    Ok(codex_home()?.join("config.toml"))
}

fn codex_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    Ok(home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine your home directory"))?
        .join(".codex"))
}

/// Sidecar next to the config, so a restore never crosses filesystems.
fn backup_path(config_path: &Path) -> PathBuf {
    config_path.with_extension("toml.edgee-bak")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Spawn the app bundle's binary (not `open`) so we can watch it exit and detect a
/// single-instance handoff. Detached — own process group and no inherited stdio — so
/// it survives this command returning and the terminal closing.
fn spawn_app(binary: &Path) -> Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(binary);
    // Electron logs a firehose to stdout, and a GUI app needs no stdin.
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.kill_on_drop(false);
    cmd.spawn()
        .with_context(|| format!("failed to launch {}", binary.display()))
}

/// Resolve the ChatGPT desktop app executable.
fn app_binary() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut candidates = vec![PathBuf::from(
            "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
        )];
        if let Some(home) = home_dir() {
            candidates.push(home.join("Applications/ChatGPT.app/Contents/MacOS/ChatGPT"));
        }
        candidates.into_iter().find(|p| p.exists()).ok_or_else(|| {
            anyhow::anyhow!(
                "The ChatGPT desktop app was not found. Install it from \
                 https://chatgpt.com/download (looked in /Applications and ~/Applications)."
            )
        })
    }
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            std::env::var_os("LOCALAPPDATA")
                .map(|a| PathBuf::from(a).join("Programs\\ChatGPT\\ChatGPT.exe")),
            std::env::var_os("PROGRAMFILES").map(|a| PathBuf::from(a).join("ChatGPT\\ChatGPT.exe")),
        ];
        candidates
            .into_iter()
            .flatten()
            .find(|p| p.exists())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "The ChatGPT desktop app was not found. Install it from \
                     https://chatgpt.com/download."
                )
            })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        anyhow::bail!(
            "The ChatGPT desktop app is only available on macOS and Windows."
        )
    }
}

fn print_launch_hint(base_url: &str, session_id: &str) {
    println!(
        "{}",
        style("Launching the ChatGPT desktop app through Edgee.").bold()
    );
    println!("  {} {}", style("gateway:").dim(), style(base_url).cyan());
    println!("  {} {}", style("session:").dim(), session_id);
    println!(
        "  {}",
        style("Quit any running ChatGPT app first — the config is only picked up by a").dim()
    );
    println!("  {}", style("freshly started instance.").dim());
    println!();
}

/// Printed once the app has taken the config and the user's file is back. The app
/// keeps running; this command is done.
fn print_handoff_complete(creds: &crate::config::Credentials, session_id: &str) {
    println!(
        "  {}",
        style("Your codex config has been restored — the app keeps the settings in memory.").dim()
    );
    println!(
        "  {}",
        style("You can close this terminal; the app keeps running.").dim()
    );
    println!(
        "  {} {}",
        style("Usage & compression stats:").dim(),
        style(crate::commands::util::session_log::logs_url_for_session(
            creds, session_id
        ))
        .cyan()
        .underlined()
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> ProviderBlock {
        render_provider_block(
            "https://edgee.io/v1",
            "sk-edgee-test",
            "session-123",
            None,
            None,
        )
    }

    fn parse(text: &str) -> toml::Table {
        text.parse::<toml::Table>().expect("patched config parses")
    }

    #[test]
    fn patch_registers_the_edgee_provider() {
        let out = patch_config_text("", &block());
        let doc = parse(&out);
        assert_eq!(doc["model_provider"].as_str(), Some(PROVIDER_ID));
        let provider = &doc["model_providers"][PROVIDER_ID];
        assert_eq!(provider["base_url"].as_str(), Some("https://edgee.io/v1"));
        assert_eq!(provider["wire_api"].as_str(), Some("responses"));
        let headers = &provider["http_headers"];
        assert_eq!(headers["x-edgee-api-key"].as_str(), Some("sk-edgee-test"));
        assert_eq!(headers["x-edgee-session-id"].as_str(), Some("session-123"));
    }

    // Placement regression: a scalar after our tables gets absorbed into them.
    #[test]
    fn user_top_level_keys_survive_and_stay_top_level() {
        let existing = "model = \"gpt-5.5\"\nmodel_reasoning_effort = \"medium\"\n";
        let doc = parse(&patch_config_text(existing, &block()));
        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("medium"));
        assert!(
            !doc["model_providers"][PROVIDER_ID]["http_headers"]
                .as_table()
                .unwrap()
                .contains_key("model"),
            "a user scalar leaked into our headers table"
        );
    }

    #[test]
    fn existing_tables_are_preserved() {
        let existing = concat!(
            "model = \"gpt-5.5\"\n",
            "\n[projects.\"/Users/me/work\"]\n",
            "trust_level = \"trusted\"\n",
            "\n[mcp_servers.node_repl]\n",
            "command = \"/bin/node_repl\"\n",
        );
        let doc = parse(&patch_config_text(existing, &block()));
        assert_eq!(
            doc["projects"]["/Users/me/work"]["trust_level"].as_str(),
            Some("trusted")
        );
        assert_eq!(
            doc["mcp_servers"]["node_repl"]["command"].as_str(),
            Some("/bin/node_repl")
        );
        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
    }

    // Duplicate keys/tables are invalid TOML, so re-patching must replace, not stack.
    #[test]
    fn patching_twice_replaces_rather_than_duplicates() {
        let once = patch_config_text("model = \"gpt-5.5\"\n", &block());
        let twice = patch_config_text(&once, &block());
        let doc = parse(&twice); // would fail outright on a duplicate key
        assert_eq!(doc["model_provider"].as_str(), Some(PROVIDER_ID));
        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(twice.matches("model_provider = ").count(), 1);
        assert_eq!(
            twice
                .matches(&format!("[model_providers.{PROVIDER_ID}]"))
                .count(),
            1
        );
        let thrice = patch_config_text(&twice, &block());
        assert_eq!(parse(&thrice)["model"].as_str(), Some("gpt-5.5"));
    }

    // Semantic equality: incidental blank lines don't matter.
    #[test]
    fn stripping_our_block_leaves_the_users_config() {
        let original = "model = \"gpt-5.5\"\n\n[projects.\"/w\"]\ntrust_level = \"trusted\"\n";
        let stripped = strip_managed_blocks(&patch_config_text(original, &block()));
        assert_eq!(parse(&stripped), parse(original));
        assert!(!stripped.contains(PROVIDER_ID));
    }

    #[test]
    fn an_existing_selector_is_replaced_not_appended() {
        let existing = "model_provider = \"mine\"\nmodel = \"gpt-5.5\"\n";
        let doc = parse(&patch_config_text(existing, &block()));
        assert_eq!(doc["model_provider"].as_str(), Some(PROVIDER_ID));
    }

    #[test]
    fn optional_headers_are_included_when_present() {
        let b = render_provider_block(
            "https://edgee.io/v1",
            "k",
            "s",
            Some("git@github.com:edgee-ai/edgee.git"),
            None,
        );
        let doc = parse(&patch_config_text("", &b));
        assert_eq!(
            doc["model_providers"][PROVIDER_ID]["http_headers"]["x-edgee-repo"].as_str(),
            Some("git@github.com:edgee-ai/edgee.git")
        );
    }

    // A quote in a repo URL must not break out of the TOML string.
    #[test]
    fn values_are_escaped_into_the_toml_string() {
        let b = render_provider_block("https://edgee.io/v1", "k", "s", Some("a\"b\\c"), None);
        let doc = parse(&patch_config_text("", &b));
        assert_eq!(
            doc["model_providers"][PROVIDER_ID]["http_headers"]["x-edgee-repo"].as_str(),
            Some("a\"b\\c")
        );
    }

    #[test]
    fn backup_sits_next_to_the_config() {
        let cfg = PathBuf::from("/home/me/.codex/config.toml");
        assert_eq!(
            backup_path(&cfg),
            PathBuf::from("/home/me/.codex/config.toml.edgee-bak")
        );
    }

    #[test]
    fn restore_puts_the_original_back_and_removes_the_backup() {
        let dir = std::env::temp_dir().join(format!("edgee-cxd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let backup = backup_path(&cfg);
        let original = "model = \"gpt-5.5\"\n";
        std::fs::write(&cfg, original).unwrap();

        apply_provider_block(&cfg, &backup, &block()).unwrap();
        assert!(backup.exists(), "backup should exist while patched");
        assert!(std::fs::read_to_string(&cfg).unwrap().contains(PROVIDER_ID));

        restore_config(&cfg, &backup).unwrap();
        // Semantic, not byte-exact: restore strips rather than rolls back.
        let after = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(parse(&after), parse(original));
        assert!(!after.contains(PROVIDER_ID));
        assert!(!backup.exists(), "backup should be cleaned up");

        std::fs::remove_dir_all(&dir).ok();
    }

    // The reason restore strips instead of rolling back: codex writes to this file
    // while the app runs, and those writes must survive.
    #[test]
    fn restore_keeps_what_the_app_wrote_while_running() {
        let dir = std::env::temp_dir().join(format!("edgee-cxd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let backup = backup_path(&cfg);
        std::fs::write(&cfg, "model = \"gpt-5.5\"\n\n[projects.\"/a\"]\ntrust_level = \"trusted\"\n")
            .unwrap();

        apply_provider_block(&cfg, &backup, &block()).unwrap();

        // codex trusts a second project mid-session.
        let patched = std::fs::read_to_string(&cfg).unwrap();
        std::fs::write(
            &cfg,
            format!("{patched}\n[projects.\"/b\"]\ntrust_level = \"trusted\"\n"),
        )
        .unwrap();

        restore_config(&cfg, &backup).unwrap();

        let doc = parse(&std::fs::read_to_string(&cfg).unwrap());
        assert_eq!(doc["projects"]["/a"]["trust_level"].as_str(), Some("trusted"));
        assert_eq!(
            doc["projects"]["/b"]["trust_level"].as_str(),
            Some("trusted"),
            "a project trusted during the session was discarded"
        );
        assert!(!doc.contains_key("model_provider"), "our provider survived");
        assert!(!doc.contains_key("model_providers"), "our tables survived");
        assert!(!backup.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    // If codex reserializes and drops our marker comments, fall back to the snapshot
    // rather than leave a live key behind.
    #[test]
    fn restore_falls_back_to_the_backup_when_markers_are_lost() {
        let dir = std::env::temp_dir().join(format!("edgee-cxd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let backup = backup_path(&cfg);
        let original = "model = \"gpt-5.5\"\n";
        std::fs::write(&cfg, original).unwrap();

        apply_provider_block(&cfg, &backup, &block()).unwrap();
        // Provider present, markers gone.
        let no_markers: String = std::fs::read_to_string(&cfg)
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(&cfg, &no_markers).unwrap();
        assert!(no_markers.contains(PROVIDER_ID));

        restore_config(&cfg, &backup).unwrap();

        let after = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(after, original);
        assert!(
            !after.contains(PROVIDER_ID),
            "must never leave the Edgee provider behind"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // No config before us → none left behind.
    #[test]
    fn restore_removes_a_config_we_created() {
        let dir = std::env::temp_dir().join(format!("edgee-cxd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let backup = backup_path(&cfg);

        apply_provider_block(&cfg, &backup, &block()).unwrap();
        assert!(cfg.exists());

        restore_config(&cfg, &backup).unwrap();
        assert!(!cfg.exists(), "should not leave a config we invented");

        std::fs::remove_dir_all(&dir).ok();
    }
}
