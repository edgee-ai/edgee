//! `edgee launch codex-desktop` — the ChatGPT / Codex desktop app through Edgee.
//!
//! Unlike the other app targets this needs **no relay**. The desktop app's backend
//! is a bundled `codex app-server` (`ChatGPT.app/Contents/Resources/codex`) that
//! reads the ordinary `$CODEX_HOME/config.toml`, and it honors `base_url` and
//! `http_headers` from a custom `model_providers` entry — the same settings
//! [`super::codex`] passes the CLI as `-c` overrides.
//!
//! Two constraints shape the implementation:
//!
//! * **The config file is the only lever.** The app spawns its codex with its own
//!   argv (`-c features.code_mode_host=true app-server`), so we cannot inject `-c`
//!   or `--profile` the way the CLI target does.
//! * **`CODEX_HOME` must not be redirected to a private copy.** It does propagate
//!   into the spawned child, but a second home means a second `auth.json`, and the
//!   ChatGPT OAuth *refresh* token is single-use/rotating: whichever copy refreshes
//!   first invalidates the other, and the user gets "your refresh token was already
//!   used. Please log out and sign in again."
//!
//! So we patch the real `config.toml` in place, back it up, and restore it when the
//! app exits. `auth.json` is never read, copied, moved or symlinked.
//!
//! While the app runs, the user's own `codex` CLI reads the same patched config and
//! is therefore also routed through Edgee. That is inherent to sharing one codex
//! home and is why the patch is reverted on exit rather than left behind.

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

    let spawned_at = std::time::Instant::now();
    let mut child = match spawn_app(&binary) {
        Ok(child) => child,
        Err(e) => {
            // Never leave the user's config patched if the app did not start.
            let _ = restore_config(&config_path, &backup);
            return Err(e);
        }
    };

    // `std::process::exit` skips `Drop`, so every exit path restores explicitly
    // rather than relying on a guard.
    let mut wait = Box::pin(child.wait());
    let status = tokio::select! {
        r = &mut wait => r.context("waiting for the ChatGPT desktop app")?,
        _ = tokio::signal::ctrl_c() => {
            drop(wait);
            // On macOS a terminal Ctrl-C reaches the whole foreground process group,
            // so the app may already be quitting; give it a moment, then force it.
            // Otherwise it would keep running against a config we are about to revert.
            if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
                .await
                .is_err()
            {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            let _ = restore_config(&config_path, &backup);
            std::process::exit(130);
        }
    };
    drop(wait);

    restore_config(&config_path, &backup)?;

    // The desktop app holds a single-instance lock: when one was already running,
    // the binary we spawned hands off to it and exits ~immediately with success —
    // and that pre-existing instance loaded the *unpatched* config, so it is not
    // going through Edgee. Report it instead of exiting 0 as if it were.
    if status.success() && spawned_at.elapsed() < std::time::Duration::from_millis(1500) {
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

    super::print_session_stats(&creds, &session_id, "ChatGPT Desktop").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
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

/// Put the user's config back and drop the backup. Idempotent: a missing backup
/// means there is nothing to restore.
fn restore_config(config_path: &Path, backup: &Path) -> Result<()> {
    if !backup.exists() {
        return Ok(());
    }
    let original = std::fs::read_to_string(backup)
        .with_context(|| format!("reading config backup {}", backup.display()))?;
    if original.is_empty() {
        // There was no config before we started; leave none behind.
        let _ = std::fs::remove_file(config_path);
    } else {
        std::fs::write(config_path, original)
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

/// Spawn the app bundle's own binary rather than `open`, so this process stays
/// attached for the app's lifetime (which is what bounds the config patch).
fn spawn_app(binary: &Path) -> Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(binary);
    // Electron GUI apps launched from a terminal spew browser-process logs; none of
    // it is actionable here, and a GUI app needs no stdin.
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null());
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
    println!(
        "  {}",
        style("freshly started instance. Your codex config is restored on exit.").dim()
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
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), original);
        assert!(!backup.exists(), "backup should be cleaned up");

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
