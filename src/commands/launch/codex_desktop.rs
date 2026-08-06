//! `edgee launch codex-desktop` — the ChatGPT / Codex desktop app through Edgee.
//!
//! Unlike the other app targets this needs **no relay**. The desktop app's backend
//! is a bundled `codex app-server` (`ChatGPT.app/Contents/Resources/codex`) that
//! reads the ordinary `$CODEX_HOME/config.toml`, and it honors `base_url` and
//! `http_headers` from a custom `model_providers` entry — the same settings
//! [`super::codex`] passes the CLI as `-c` overrides.
//!
//! Three constraints shape the implementation, each established empirically:
//!
//! * **The config file is the only lever.** The app spawns its codex with its own
//!   argv (`-c features.code_mode_host=true app-server`), so we cannot inject `-c`.
//!   `--profile <name>` would be ideal — it layers `$CODEX_HOME/<name>.config.toml`
//!   over the base config, touching nothing of the user's — but it is argv-only:
//!   there is no `CODEX_PROFILE` env var, and the legacy `profile = "…"` config key
//!   is a hard startup error in 0.147 ("no longer supported; use `--profile`").
//! * **`CODEX_HOME` must not be redirected to a private copy.** It does propagate
//!   into the spawned child, but a second home means a second `auth.json`, and the
//!   ChatGPT OAuth *refresh* token is single-use/rotating: whichever copy refreshes
//!   first invalidates the other, and the user gets "your refresh token was already
//!   used. Please log out and sign in again." Symlinking or hardlinking it does not
//!   help — codex removes and atomically replaces that file. Nor can auth come from
//!   the environment: `CODEX_ACCESS_TOKEN` is an agent-identity slot, not a ChatGPT
//!   one, so it would force API-key billing instead of the user's plan.
//! * **The app parses the config once, at startup, and keeps it in memory.** So the
//!   patch only has to survive the handoff.
//!
//! That last point is what shapes the lifecycle: patch `config.toml`, spawn the app
//! **detached**, wait out [`CONFIG_HANDOFF_GRACE`], revert, and return. The command
//! exits in ~10s while the app keeps running with the settings cached, so the user's
//! `codex` CLI is unaffected from then on, the terminal is free to close, and no
//! crash window can strand the patch. `auth.json` is never read, copied, moved or
//! symlinked.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use console::style;

use super::util;

/// The provider id we register in the user's codex config. Matches the CLI target
/// so a session looks the same from the gateway's side.
const PROVIDER_ID: &str = "edgee-cli";

/// Markers bracketing everything this command inserts. They make the patch
/// removable by inspection — `restore_config` normally puts the user's file back
/// wholesale, but if a backup is ever lost these let us strip a previous insertion
/// instead of appending a second, conflicting copy (duplicate `model_provider`, or
/// a duplicate `[model_providers.…]` table, which is invalid TOML).
const MANAGED_BEGIN: &str = "# >>> edgee managed block — removed when the app exits";
const MANAGED_END: &str = "# <<< edgee managed block";

#[derive(Debug, clap::Parser)]
pub struct Options {}

pub async fn run(_opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Auth bootstrap — identical to the `codex` CLI target, and it shares the
    // `codex` provider key (one Edgee agent covers both surfaces).
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }
    crate::commands::auth::login::ensure_org_selected().await?;
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("codex")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("codex").await?;
    }
    creds = crate::config::read()?;

    let api_key = creds
        .codex
        .as_ref()
        .map(|c| c.api_key.clone())
        .ok_or_else(|| anyhow::anyhow!("no Edgee API key for 'codex'; run `edgee auth login`"))?;
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

    // A leftover backup means a previous run died before restoring (crash, SIGKILL).
    // Put the user's file back before taking a new backup, or we would snapshot our
    // own patched config and "restore" to it forever.
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
            // Never leave the user's config patched if the app did not start.
            let _ = restore_config(&config_path, &backup);
            return Err(e);
        }
    };

    // The app's bundled codex parses config.toml once at startup and keeps it in
    // memory for the session, so the patch only has to survive the handoff — a few
    // seconds — not the whole session. Waiting it out and reverting immediately is
    // what lets this command exit while the app keeps running.
    let quick_exit = wait_out_config_handoff(&mut child).await;

    // Back to the user's own config before we return, so nothing outlives this
    // command: the `codex` CLI is unaffected from here on, and there is no window in
    // which a crash could strand the patch.
    restore_config(&config_path, &backup)?;

    // The desktop app holds a single-instance lock: when one was already running,
    // the binary we spawned hands off to it and exits ~immediately with success —
    // and that pre-existing instance loaded the *unpatched* config, so it is not
    // going through Edgee. Report it instead of exiting 0 as if it were.
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

/// How long to leave the patch in place after spawning the app, so its bundled
/// codex can start and read `config.toml`.
///
/// Measured handoff is ~2s to spawn the child plus a moment to parse; 10s is a
/// deliberately generous margin for a cold start. The failure mode if the app were
/// somehow slower is benign: it reads the already-restored config and simply talks
/// to OpenAI directly — no compression and no metering, but nothing broken.
const CONFIG_HANDOFF_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// A GUI app that exits this fast did not really start: it hit the single-instance
/// lock and handed off to a process that already had the unpatched config.
const HANDOFF_EXIT_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);

/// Hold the patch for [`CONFIG_HANDOFF_GRACE`], then report whether the app exited
/// almost immediately (the single-instance handoff case).
///
/// Polls rather than sleeping straight through so the common handoff case is caught
/// early instead of making the user wait out the full grace period.
async fn wait_out_config_handoff(child: &mut tokio::process::Child) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < CONFIG_HANDOFF_GRACE {
        match child.try_wait() {
            // Exited already — only a near-instant success means "handed off".
            Ok(Some(status)) => {
                return status.success() && started.elapsed() < HANDOFF_EXIT_WINDOW;
            }
            Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            // Can't tell; treat as still running and let the grace period elapse.
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
        }
    }
    false
}

/// Escape a value for a double-quoted TOML basic string.
fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The two TOML fragments to splice into the user's config: the top-level
/// `model_provider` selector, and the `[model_providers.*]` tables it points at.
///
/// They are returned separately because they cannot be inserted at the same place —
/// see [`apply_provider_block`].
struct ProviderBlock {
    selector: String,
    tables: String,
}

/// Build the Edgee provider definition. Mirrors the `-c` overrides in
/// [`super::codex`]: same provider id, same `wire_api`, same `x-edgee-*` headers.
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

/// Remove every previously inserted managed block, so patching is idempotent.
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

/// Splice `block` into the config text, preserving everything already there.
///
/// TOML placement is load-bearing and the two fragments go to different places:
///
/// * `model_provider` is a top-level scalar, so it must land **before the first
///   table header** — after one, it would be read as a member of that table.
/// * the `[model_providers.…]` tables must land **after all top-level scalars**, or
///   the user's own bare keys (`model = "gpt-5.5"`) would be absorbed into our
///   table.
///
/// Inserting the selector at the first table header and the tables immediately
/// after it satisfies both, and is why this is textual rather than a
/// parse-and-reserialize (which would drop the user's comments and formatting).
fn patch_config_text(existing: &str, block: &ProviderBlock) -> String {
    // Drop any leftover insertion of our own first, so re-patching replaces rather
    // than stacking (a second `model_provider`, or a duplicate provider table).
    let existing = strip_managed_blocks(existing);

    let first_table = existing
        .lines()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(usize::MAX);

    let mut head = String::new();
    let mut tail = String::new();
    for (i, line) in existing.lines().enumerate() {
        // Drop any pre-existing selector so ours is unambiguous; the user's own
        // choice is preserved in the backup and restored on exit.
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
    // A missing config is fine — the app writes one on first run, and an empty base
    // is a valid starting point for the patch.
    let existing = std::fs::read_to_string(config_path).unwrap_or_default();

    std::fs::write(backup, &existing)
        .with_context(|| format!("writing config backup {}", backup.display()))?;

    let patched = patch_config_text(&existing, block);
    std::fs::write(config_path, patched)
        .with_context(|| format!("patching {}", config_path.display()))?;
    Ok(())
}

/// Remove our managed blocks from the config, leaving everything else as it is
/// **now** — not as it was when we patched.
///
/// This is deliberately surgical rather than a rollback to the backup. Codex
/// rewrites `config.toml` while it runs (project `trust_level`s, plugin enable
/// state, marketplace timestamps, the `[desktop]` block), so restoring the
/// snapshot would silently discard everything the user did in the app that
/// session — trust a new project, quit, lose the trust.
///
/// The backup is kept only as a fallback for the case where our markers are gone
/// (see below) and as crash recovery for the next run. Idempotent: no backup means
/// nothing to undo.
fn restore_config(config_path: &Path, backup: &Path) -> Result<()> {
    if !backup.exists() {
        return Ok(());
    }
    let backup_body = std::fs::read_to_string(backup)
        .with_context(|| format!("reading config backup {}", backup.display()))?;
    let current = std::fs::read_to_string(config_path).unwrap_or_default();

    if current.contains(MANAGED_BEGIN) {
        let stripped = strip_managed_blocks(&current);
        // An empty backup means the file did not exist before us. If stripping
        // leaves nothing of substance, leave no file behind rather than an empty one.
        if backup_body.is_empty() && stripped.trim().is_empty() {
            let _ = std::fs::remove_file(config_path);
        } else {
            std::fs::write(config_path, stripped)
                .with_context(|| format!("restoring {}", config_path.display()))?;
        }
    } else if backup_body.is_empty() {
        // We created the file and cannot find our markers — don't guess at which
        // parts are ours; the file is ours to remove.
        let _ = std::fs::remove_file(config_path);
    } else {
        // Markers gone but the file existed before us: codex may have reserialized
        // the file and dropped comments (our markers ARE comments). We can no
        // longer tell our lines from the user's, so fall back to the snapshot. That
        // loses this session's runtime writes, but never leaves Edgee's provider —
        // and a stale key pointed at a gateway is the worse failure.
        std::fs::write(config_path, &backup_body)
            .with_context(|| format!("restoring {}", config_path.display()))?;
    }

    let _ = std::fs::remove_file(backup);
    Ok(())
}

/// `$CODEX_HOME/config.toml`, honoring the same `CODEX_HOME` override codex itself
/// reads so a user who relocated their codex home still gets patched correctly.
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

/// Sidecar next to the config so both live on one filesystem — a rename/restore
/// never crosses devices, and it is obvious what it belongs to.
fn backup_path(config_path: &Path) -> PathBuf {
    config_path.with_extension("toml.edgee-bak")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Spawn the app bundle's own binary rather than `open`, so the config patch is
/// guaranteed to be in place before the process starts and we can observe a
/// single-instance handoff by watching it exit.
///
/// **Detached**: the app is put in its own process group and does not inherit this
/// terminal's stdio, so it survives both this command returning and the terminal
/// being closed. Nothing about the running app depends on `edgee` afterwards — the
/// config has already been reverted by then.
fn spawn_app(binary: &Path) -> Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(binary);
    // Electron GUI apps launched from a terminal spew browser-process logs; none of
    // it is actionable here, and a GUI app needs no stdin. Detaching stdio also means
    // a closed terminal cannot write-fail the app.
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null());
    // Own process group: a terminal SIGINT/SIGHUP goes to the foreground group, and
    // we do not want Ctrl-C or a closed window to take the app down with us.
    #[cfg(unix)]
    cmd.process_group(0);
    // Windows equivalent: detach from this console so it survives its parent.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    // Don't reap-block on this child at exit; we deliberately outlive it.
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

    // The whole point of the target: the app reads these three settings.
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

    // TOML placement regression: a top-level scalar AFTER our tables would be
    // silently absorbed into `[model_providers.edgee-cli.http_headers]`, so the
    // user's `model` would vanish and become a bogus header.
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

    // Existing tables (trust levels, mcp servers, plugins) must come through
    // untouched — this config is the user's real working state.
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

    // Re-patching an already-patched file (reachable if a backup is lost) must
    // REPLACE the previous insertion, not stack a second one: two `model_provider`
    // keys or two `[model_providers.edgee-cli]` tables are both invalid TOML.
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
        // And a third time stays stable.
        let thrice = patch_config_text(&twice, &block());
        assert_eq!(parse(&thrice)["model"].as_str(), Some("gpt-5.5"));
    }

    // Stripping our block must leave a config semantically identical to the user's
    // (blank-line differences are irrelevant — exact text restoration is the
    // backup's job, not the stripper's).
    #[test]
    fn stripping_our_block_leaves_the_users_config() {
        let original = "model = \"gpt-5.5\"\n\n[projects.\"/w\"]\ntrust_level = \"trusted\"\n";
        let stripped = strip_managed_blocks(&patch_config_text(original, &block()));
        assert_eq!(parse(&stripped), parse(original));
        assert!(!stripped.contains(PROVIDER_ID));
    }

    // A user who pinned their own provider gets ours while the app runs, and their
    // original back from the backup on exit.
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

    // Values reach a TOML string literal, so a quote in a repo URL must not be able
    // to break out of it.
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
        // Semantic, not byte-exact: restore strips our block from the live file
        // rather than rolling back to the snapshot, so incidental blank lines differ.
        let after = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(parse(&after), parse(original));
        assert!(!after.contains(PROVIDER_ID));
        assert!(!backup.exists(), "backup should be cleaned up");

        std::fs::remove_dir_all(&dir).ok();
    }

    // The regression this function exists for: codex rewrites config.toml while the
    // app runs, and restore must KEEP those writes while removing only our block.
    // A rollback to the backup would silently drop the newly trusted project.
    #[test]
    fn restore_keeps_what_the_app_wrote_while_running() {
        let dir = std::env::temp_dir().join(format!("edgee-cxd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let backup = backup_path(&cfg);
        std::fs::write(&cfg, "model = \"gpt-5.5\"\n\n[projects.\"/a\"]\ntrust_level = \"trusted\"\n")
            .unwrap();

        apply_provider_block(&cfg, &backup, &block()).unwrap();

        // Simulate codex trusting a second project mid-session.
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

    // Markers are comments, so a reserialization by codex could drop them. We then
    // cannot separate our lines from the user's — fall back to the snapshot rather
    // than leave a live Edgee key pointed at a gateway.
    #[test]
    fn restore_falls_back_to_the_backup_when_markers_are_lost() {
        let dir = std::env::temp_dir().join(format!("edgee-cxd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        let backup = backup_path(&cfg);
        let original = "model = \"gpt-5.5\"\n";
        std::fs::write(&cfg, original).unwrap();

        apply_provider_block(&cfg, &backup, &block()).unwrap();
        // Reserialized without comments: provider present, markers gone.
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

    // No config before we started → none left behind, rather than a file containing
    // only Edgee's provider.
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
