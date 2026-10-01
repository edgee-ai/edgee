//! `edgee launch pi`: Pi CLI (<https://pi.dev>) through the `pi-edgee` extension.
//!
//! Pi no longer gets provider blocks written into its `models.json`. The
//! first-party `pi-edgee` extension registers the `edgee` provider, discovers
//! models, renders the footer and syncs session metadata; this launcher loads
//! it for the run and hands it the identity the CLI already selected.
//!
//! ## Delivery: per-launch `-e`, no install
//!
//! The extension is passed as `-e npm:pi-edgee@<pinned>`. Pi treats that as a
//! temporary source: it caches the package under `<agent dir>/tmp/extensions`
//! and never touches `settings.json`, so plain `pi` is unaffected and nothing
//! has to be confirmed. When the user already has pi-edgee in their own agent
//! dir (settings `packages` or `extensions/`), injecting a second copy would
//! register `edgee` twice (Pi dedupes by canonical path only), so the launch
//! reuses theirs and warns if it predates the CLI contract.
//!
//! Detection is deliberately shallow: the user-level agent dir, plus the
//! user's own `-e`. A project `.pi/settings.json` is only read once the
//! project is trusted, so skipping injection because of it could leave a run
//! with no Edgee provider at all.
//!
//! ## Ephemeral identity
//!
//! The gateway key, console token, org, endpoints and debug headers travel in
//! one child-only JSON env var ([`CONTEXT_ENV`]), never in argv, settings or
//! Pi's `auth.json`. The extension reads it once and scrubs it from its own
//! environment. `EDGEE_API_KEY`/`EDGEE_SESSION_ID` are still exported so a
//! user-installed pi-edgee that predates the contract keeps working.
//!
//! OMP does not use this path; it keeps its provider-file launcher in `omp.rs`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use serde_json::Value;

use super::util;
use crate::commands::util::plugins;

/// npm release of `pi-edgee` that implements [`CONTRACT_VERSION`]. A CLI
/// release must not pin a version that is not published yet.
const PI_EDGEE_SPEC: &str = "npm:pi-edgee@0.2.0";

/// Local checkout or alternative spec to load instead of [`PI_EDGEE_SPEC`].
const EXTENSION_OVERRIDE_ENV: &str = "EDGEE_PI_EXTENSION";

/// Child-only env var carrying the [`LaunchContext`] JSON.
const CONTEXT_ENV: &str = "EDGEE_PI_CONTEXT";

/// `version` written into the context and the minimum `edgee.cliContract` a
/// user-installed pi-edgee must advertise in its `package.json`.
const CONTRACT_VERSION: u64 = 1;

/// Provider keys the previous launcher wrote under `providers` in `models.json`.
const LEGACY_PROVIDER_KEYS: [&str; 2] = ["edgee", "edgee-anthropic"];

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the agent CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Pi's agent directory, mirroring its own resolution order: an explicit
/// `PI_CODING_AGENT_DIR` (tilde-expanded, as Pi does) wins over `~/.pi/agent`.
fn agent_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var("PI_CODING_AGENT_DIR")
        .ok()
        .filter(|d| !d.is_empty())
    {
        if dir == "~" {
            return home_dir();
        }
        if let Some(rest) = dir.strip_prefix("~/") {
            return home_dir().map(|h| h.join(rest));
        }
        return Some(PathBuf::from(dir));
    }
    home_dir().map(|h| h.join(".pi").join("agent"))
}

/// Everything pi-edgee needs to act as the CLI-selected identity. No `Debug`:
/// it holds credentials.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchContext {
    version: u64,
    session_id: String,
    api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key_id: Option<String>,
    user_token: String,
    org_id: String,
    org_slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_name: Option<String>,
    gateway_url: String,
    console_url: String,
    console_api_url: String,
    mcp_url: String,
    mcp_disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    debug_headers: Option<DebugHeaders>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugHeaders {
    pubkey: String,
    salt: String,
}

/// Whether to add `-e <spec>`, and what to tell the user about their own copy.
#[derive(Debug, PartialEq, Eq)]
struct ExtensionPlan {
    inject: Option<String>,
    warning: Option<String>,
}

/// A pi-edgee the user installed themselves. `manifest` is its `package.json`
/// when we can point at one.
struct UserInstall {
    manifest: Option<PathBuf>,
}

/// `-e`/`--extension` values in `args`, the same two spellings Pi's parser
/// accepts (it has no `--extension=value` form).
fn extension_args(args: &[String]) -> impl Iterator<Item = &str> {
    args.windows(2)
        .filter(|pair| pair[0] == "-e" || pair[0] == "--extension")
        .map(|pair| pair[1].as_str())
}

fn is_pi_edgee_npm(source: &str) -> bool {
    source
        .strip_prefix("npm:")
        .and_then(|spec| spec.split('@').next())
        == Some("pi-edgee")
}

fn package_name(manifest: &Path) -> Option<String> {
    let parsed: Value = serde_json::from_str(&std::fs::read_to_string(manifest).ok()?).ok()?;
    parsed.get("name")?.as_str().map(str::to_string)
}

/// Resolves a local `packages` source the way Pi reads it from the user's
/// `settings.json`: `~/` from the home dir, relative paths from the agent dir.
fn resolve_local_source(source: &str, agent_dir: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = source.strip_prefix("~/") {
        return home.map(|h| h.join(rest));
    }
    let path = Path::new(source);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else if source.starts_with('.') {
        Some(agent_dir.join(path))
    } else {
        None
    }
}

/// Looks for pi-edgee in the user's agent dir: a `packages` entry in
/// `settings.json` (npm or local) or an auto-discovered `extensions/` entry.
/// Anything it does not recognise counts as not installed.
fn find_user_install(agent_dir: &Path, home: Option<&Path>) -> Option<UserInstall> {
    if let Some(settings) = std::fs::read_to_string(agent_dir.join("settings.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    {
        for entry in settings
            .get("packages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            // The object form carries per-resource filters; an empty
            // `extensions` list loads nothing from the package.
            let source = match entry {
                Value::String(source) => source.as_str(),
                Value::Object(object) => {
                    if object
                        .get("extensions")
                        .and_then(Value::as_array)
                        .is_some_and(|list| list.is_empty())
                    {
                        continue;
                    }
                    match object.get("source").and_then(Value::as_str) {
                        Some(source) => source,
                        None => continue,
                    }
                }
                _ => continue,
            };
            if is_pi_edgee_npm(source) {
                return Some(UserInstall {
                    manifest: Some(
                        agent_dir
                            .join("npm")
                            .join("node_modules")
                            .join("pi-edgee")
                            .join("package.json"),
                    ),
                });
            }
            if let Some(dir) = resolve_local_source(source, agent_dir, home) {
                let manifest = dir.join("package.json");
                if package_name(&manifest).as_deref() == Some("pi-edgee") {
                    return Some(UserInstall {
                        manifest: Some(manifest),
                    });
                }
            }
        }
    }

    let extensions = agent_dir.join("extensions");
    let dir = extensions.join("pi-edgee");
    if dir.is_dir() {
        return Some(UserInstall {
            manifest: Some(dir.join("package.json")),
        });
    }
    if extensions.join("pi-edgee.ts").is_file() {
        return Some(UserInstall { manifest: None });
    }
    None
}

/// `None` when the install advertises a compatible `edgee.cliContract`.
fn contract_warning(install: &UserInstall) -> Option<String> {
    let advertised = install
        .manifest
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|manifest| manifest.pointer("/edgee/cliContract")?.as_u64());
    if advertised.is_some_and(|version| version >= CONTRACT_VERSION) {
        return None;
    }
    Some(
        "Your own pi-edgee install does not advertise Edgee CLI support, so it was left in \
         place and the footer and session metadata may need `/login edgee`. Update it with \
         `pi update`, or remove it to let `edgee launch pi` load the matching release."
            .to_string(),
    )
}

/// Decides how pi-edgee gets loaded for this launch. Pure: paths, args and the
/// override are passed in so tests need no process-global state.
fn extension_plan(
    agent_dir: Option<&Path>,
    home: Option<&Path>,
    args: &[String],
    spec_override: Option<&str>,
) -> ExtensionPlan {
    let nothing_to_add = |warning| ExtensionPlan {
        inject: None,
        warning,
    };

    // The user already asked for pi-edgee themselves.
    if extension_args(args).any(|value| value.contains("pi-edgee")) {
        return nothing_to_add(None);
    }

    // `-ne` stops Pi loading anything it would discover, including an install.
    let discovery_disabled = args.iter().any(|a| a == "-ne" || a == "--no-extensions");
    if !discovery_disabled {
        if let Some(install) = agent_dir.and_then(|dir| find_user_install(dir, home)) {
            return nothing_to_add(contract_warning(&install));
        }
    }

    let spec = spec_override
        .filter(|s| !s.is_empty())
        .unwrap_or(PI_EDGEE_SPEC);
    ExtensionPlan {
        inject: Some(spec.to_string()),
        warning: None,
    }
}

/// What [`cleanup_legacy_providers`] did, for the caller to report.
#[derive(Debug, PartialEq, Eq)]
enum LegacyCleanup {
    Nothing,
    Removed {
        providers: Vec<String>,
        backup: PathBuf,
    },
    /// Left untouched; the string says why and what to do.
    Skipped(String),
}

/// True for a provider block exactly as the previous launcher generated it:
/// env references for the key and the session header, and a Pi transport. Any
/// customisation breaks the fingerprint and keeps the block off-limits.
fn is_cli_generated(block: &Value) -> bool {
    block.get("apiKey").and_then(Value::as_str) == Some("$EDGEE_API_KEY")
        && block
            .pointer("/headers/x-edgee-session-id")
            .and_then(Value::as_str)
            == Some("$EDGEE_SESSION_ID")
        && matches!(
            block.get("api").and_then(Value::as_str),
            Some("openai-completions" | "anthropic-messages")
        )
}

fn backup_path(models: &Path) -> PathBuf {
    let mut name = models.as_os_str().to_os_string();
    name.push(".edgee-bak");
    PathBuf::from(name)
}

/// Removes the `edgee` / `edgee-anthropic` blocks the previous launcher wrote.
///
/// Pi layers `models.json` over extension providers, so a leftover block would
/// shadow the one pi-edgee registers. Only blocks matching
/// [`is_cli_generated`] are removed, after a one-time backup, and the rewrite
/// is abandoned if the file changed in between. Unparseable files and
/// customised blocks are never touched.
fn cleanup_legacy_providers(path: &Path) -> Result<LegacyCleanup> {
    if !path.exists() {
        return Ok(LegacyCleanup::Nothing);
    }
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let Ok(mut root) = serde_json::from_str::<Value>(&original) else {
        return Ok(LegacyCleanup::Skipped(format!(
            "{} is not plain JSON, so it was left alone. If it still has `edgee` or \
             `edgee-anthropic` providers from an older `edgee launch pi`, remove them.",
            path.display()
        )));
    };
    let Some(providers) = root.get_mut("providers").and_then(Value::as_object_mut) else {
        return Ok(LegacyCleanup::Nothing);
    };

    let present: Vec<&str> = LEGACY_PROVIDER_KEYS
        .into_iter()
        .filter(|key| providers.contains_key(*key))
        .collect();
    if present.is_empty() {
        return Ok(LegacyCleanup::Nothing);
    }
    let (generated, custom): (Vec<&str>, Vec<&str>) = present
        .into_iter()
        .partition(|key| is_cli_generated(&providers[*key]));
    if generated.is_empty() {
        return Ok(LegacyCleanup::Skipped(format!(
            "{} has custom `{}` provider block(s). Pi layers models.json over extensions, so \
             they shadow pi-edgee's `edgee` provider. Remove or rename them.",
            path.display(),
            custom.join("`, `")
        )));
    }

    let backup = backup_path(path);
    if !backup.exists() {
        std::fs::copy(path, &backup)
            .with_context(|| format!("Failed to back up {}", path.display()))?;
    }
    for key in &generated {
        providers.remove(*key);
    }

    // Re-read right before writing so a concurrent edit is not overwritten.
    if std::fs::read_to_string(path).ok().as_deref() != Some(original.as_str()) {
        return Ok(LegacyCleanup::Skipped(format!(
            "{} changed while it was being cleaned up, so it was left alone.",
            path.display()
        )));
    }
    let rendered = serde_json::to_string_pretty(&root)?;
    std::fs::write(path, format!("{rendered}\n"))
        .with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(LegacyCleanup::Removed {
        providers: generated.into_iter().map(str::to_string).collect(),
        backup,
    })
}

fn report_cleanup(outcome: &LegacyCleanup) {
    match outcome {
        LegacyCleanup::Nothing => {}
        LegacyCleanup::Removed { providers, backup } => eprintln!(
            "{}",
            format!(
                "Removed the old `{}` provider block(s) from models.json (backup: {}). \
                 pi-edgee now provides `edgee`; reselect any `edgee-anthropic/...` model \
                 under it.",
                providers.join("`, `"),
                backup.display()
            )
            .dimmed()
        ),
        LegacyCleanup::Skipped(reason) => eprintln!("{}", reason.yellow()),
    }
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }
    crate::commands::auth::login::ensure_org_selected().await?;

    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("pi")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("pi").await?;
    }
    creds = crate::config::read()?;

    // The relay-style "plan" connection is what the previous launcher recorded.
    if creds
        .pi
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.pi.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    util::spawn_cli_version_report(&creds, &session_id);
    util::ensure_first_run_installed().await;

    let org = super::fetch_active_org(&creds).await;
    let gateway_url = super::gateway_base_url_with_org(org.as_ref());
    let pi_key = creds.pi.as_ref().context("Missing Pi credentials")?;
    let debug_headers = util::resolve_debug_log_keypair()?
        .map(|keypair| keypair.header_values())
        .map(|values| DebugHeaders {
            pubkey: values.pubkey,
            salt: values.salt,
        });
    let context = LaunchContext {
        version: CONTRACT_VERSION,
        session_id: session_id.clone(),
        api_key: pi_key.api_key.clone(),
        api_key_id: pi_key.api_key_id.clone(),
        user_token: creds
            .user_token
            .clone()
            .context("Missing Edgee user token")?,
        org_id: creds.org_id.clone().context("No Edgee organization selected")?,
        org_slug: creds.org_slug.clone().unwrap_or_default(),
        org_name: org.as_ref().map(|o| o.name.clone()),
        gateway_url,
        console_url: crate::config::console_base_url(),
        console_api_url: crate::config::console_api_base_url(),
        mcp_url: crate::config::mcp_base_url(),
        mcp_disabled: super::mcp_injection_disabled_with_org(org.as_ref()),
        debug_headers,
    };

    let agent_dir = agent_dir();
    if let Some(models) = agent_dir.as_ref().map(|dir| dir.join("models.json")) {
        match cleanup_legacy_providers(&models) {
            Ok(outcome) => report_cleanup(&outcome),
            // A failed cleanup must not block the launch; the user is told how to fix it.
            Err(e) => eprintln!(
                "{}",
                format!("Could not clean up old Edgee providers in models.json: {e:#}").yellow()
            ),
        }
    }

    let spec_override = std::env::var(EXTENSION_OVERRIDE_ENV).ok();
    let plan = extension_plan(
        agent_dir.as_deref(),
        home_dir().as_deref(),
        &opts.args,
        spec_override.as_deref(),
    );
    if let Some(warning) = &plan.warning {
        eprintln!("{}", warning.yellow());
    }

    let plugin_report = plugins::sync_for_target(&creds, plugins::Target::Pi).await;
    let mut cmd = std::process::Command::new(util::resolve_binary("pi"));
    cmd.env(CONTEXT_ENV, serde_json::to_string(&context)?);
    // Older pi-edgee releases only understand these.
    cmd.env("EDGEE_API_KEY", &context.api_key);
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_ORG_SLUG", &context.org_slug);
    if let Some(spec) = &plan.inject {
        cmd.args(["-e", spec]);
    }
    if let Some(path) = &plugin_report.skills_root {
        cmd.args(["--skill".to_string(), path.to_string_lossy().into_owned()]);
    }
    cmd.args(&opts.args);
    plugins::report_launch(&plugin_report);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Pi is not installed. Install it with `npm install -g @mariozechner/pi-coding-agent`"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "Pi").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn plan(dir: &Path, args: &[String]) -> ExtensionPlan {
        extension_plan(Some(dir), Some(dir), args, None)
    }

    fn manifest(contract: Option<u64>) -> String {
        match contract {
            Some(v) => format!(
                r#"{{"name":"pi-edgee","edgee":{{"cliContract":{v}}}}}"#
            ),
            None => r#"{"name":"pi-edgee"}"#.to_string(),
        }
    }

    #[test]
    fn injects_the_pinned_release_when_nothing_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan(dir.path(), &args(&["--model", "edgee/x"]));
        assert_eq!(plan.inject.as_deref(), Some(PI_EDGEE_SPEC));
        assert_eq!(plan.warning, None);
    }

    #[test]
    fn honors_the_override_for_local_checkouts() {
        let dir = tempfile::tempdir().unwrap();
        let plan = extension_plan(Some(dir.path()), None, &[], Some("/src/pi-edgee"));
        assert_eq!(plan.inject.as_deref(), Some("/src/pi-edgee"));
        let empty = extension_plan(Some(dir.path()), None, &[], Some(""));
        assert_eq!(empty.inject.as_deref(), Some(PI_EDGEE_SPEC));
    }

    #[test]
    fn injects_without_an_agent_dir() {
        let plan = extension_plan(None, None, &[], None);
        assert_eq!(plan.inject.as_deref(), Some(PI_EDGEE_SPEC));
    }

    #[test]
    fn skips_injection_when_the_user_passes_pi_edgee() {
        let dir = tempfile::tempdir().unwrap();
        for given in [
            args(&["-e", "npm:pi-edgee@0.2.0"]),
            args(&["--extension", "/src/pi-edgee"]),
            args(&["-ne", "-e", "./pi-edgee"]),
        ] {
            assert_eq!(plan(dir.path(), &given).inject, None, "{given:?}");
        }
    }

    #[test]
    fn unrelated_extensions_do_not_suppress_injection() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan(dir.path(), &args(&["-e", "npm:other-ext"]));
        assert!(plan.inject.is_some());
    }

    #[test]
    fn reuses_a_compatible_npm_install() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("settings.json"),
            r#"{"packages":["npm:pi-edgee@0.2.0"]}"#,
        );
        write(
            &dir.path().join("npm/node_modules/pi-edgee/package.json"),
            &manifest(Some(1)),
        );
        let plan = plan(dir.path(), &[]);
        assert_eq!(plan, ExtensionPlan { inject: None, warning: None });
    }

    #[test]
    fn matches_unpinned_and_object_form_sources() {
        for settings in [
            r#"{"packages":["npm:pi-edgee"]}"#,
            r#"{"packages":[{"source":"npm:pi-edgee@0.1.1","skills":[]}]}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            write(&dir.path().join("settings.json"), settings);
            assert_eq!(plan(dir.path(), &[]).inject, None, "{settings}");
        }
    }

    #[test]
    fn warns_when_an_install_predates_the_contract() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("settings.json"),
            r#"{"packages":["npm:pi-edgee@0.1.1"]}"#,
        );
        write(
            &dir.path().join("npm/node_modules/pi-edgee/package.json"),
            &manifest(None),
        );
        let plan = plan(dir.path(), &[]);
        assert_eq!(plan.inject, None);
        assert!(plan.warning.unwrap().contains("pi update"));
    }

    #[test]
    fn warns_when_the_install_cannot_be_inspected() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("settings.json"),
            r#"{"packages":["npm:pi-edgee"]}"#,
        );
        assert!(plan(dir.path(), &[]).warning.is_some());
    }

    #[test]
    fn recognises_local_checkouts_by_package_name() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("src/pi-edgee");
        write(&checkout.join("package.json"), &manifest(Some(1)));
        write(
            &dir.path().join("settings.json"),
            &format!(r#"{{"packages":["{}"]}}"#, checkout.display()),
        );
        assert_eq!(plan(dir.path(), &[]), ExtensionPlan { inject: None, warning: None });

        // Same layout but a different package: not ours.
        let other = tempfile::tempdir().unwrap();
        let unrelated = other.path().join("tools");
        write(&unrelated.join("package.json"), r#"{"name":"other"}"#);
        write(
            &other.path().join("settings.json"),
            &format!(r#"{{"packages":["{}"]}}"#, unrelated.display()),
        );
        assert!(plan(other.path(), &[]).inject.is_some());
    }

    #[test]
    fn resolves_tilde_and_relative_local_sources() {
        let home = tempfile::tempdir().unwrap();
        let agent = home.path().join(".pi/agent");
        write(&home.path().join("dev/pi-edgee/package.json"), &manifest(Some(1)));
        write(&agent.join("settings.json"), r#"{"packages":["~/dev/pi-edgee"]}"#);
        let tilde = extension_plan(Some(&agent), Some(home.path()), &[], None);
        assert_eq!(tilde.inject, None);

        write(&agent.join("local/pi-edgee/package.json"), &manifest(Some(1)));
        write(&agent.join("settings.json"), r#"{"packages":["./local/pi-edgee"]}"#);
        let relative = extension_plan(Some(&agent), Some(home.path()), &[], None);
        assert_eq!(relative.inject, None);
    }

    #[test]
    fn a_filtered_out_package_does_not_count_as_installed() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("settings.json"),
            r#"{"packages":[{"source":"npm:pi-edgee","extensions":[]}]}"#,
        );
        assert!(plan(dir.path(), &[]).inject.is_some());
    }

    #[test]
    fn recognises_the_auto_discovered_extensions_dir() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("extensions/pi-edgee/package.json"),
            &manifest(Some(1)),
        );
        assert_eq!(plan(dir.path(), &[]).inject, None);

        let single = tempfile::tempdir().unwrap();
        write(&single.path().join("extensions/pi-edgee.ts"), "export default () => {}");
        let plan = plan(single.path(), &[]);
        assert_eq!(plan.inject, None);
        assert!(plan.warning.is_some());
    }

    #[test]
    fn no_extensions_ignores_installs_and_injects_anyway() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("settings.json"),
            r#"{"packages":["npm:pi-edgee"]}"#,
        );
        for flag in ["-ne", "--no-extensions"] {
            assert!(plan(dir.path(), &args(&[flag])).inject.is_some(), "{flag}");
        }
    }

    #[test]
    fn malformed_settings_count_as_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("settings.json"), "{not json");
        assert!(plan(dir.path(), &[]).inject.is_some());
    }

    #[test]
    fn project_level_installs_are_not_considered() {
        let dir = tempfile::tempdir().unwrap();
        let agent = dir.path().join("agent");
        std::fs::create_dir_all(&agent).unwrap();
        write(
            &dir.path().join(".pi/settings.json"),
            r#"{"packages":["npm:pi-edgee"]}"#,
        );
        assert!(plan(&agent, &[]).inject.is_some());
    }

    #[test]
    fn extension_args_follow_pi_flag_spelling() {
        let given = args(&["--extension=npm:pi-edgee", "-e", "a", "--extension", "b"]);
        assert_eq!(extension_args(&given).collect::<Vec<_>>(), ["a", "b"]);
    }

    fn cli_block(api: &str) -> Value {
        serde_json::json!({
            "name": "Edgee",
            "api": api,
            "apiKey": "$EDGEE_API_KEY",
            "headers": {
                "x-edgee-api-key": "$EDGEE_API_KEY",
                "x-edgee-session-id": "$EDGEE_SESSION_ID"
            },
            "models": [{ "id": "openai/gpt-5" }]
        })
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn removes_generated_blocks_and_keeps_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let original = serde_json::json!({
            "providers": {
                "edgee": cli_block("openai-completions"),
                "edgee-anthropic": cli_block("anthropic-messages"),
                "ollama": { "baseUrl": "http://localhost:11434/v1" }
            },
            "other": [1, 2, 3]
        });
        write(&path, &serde_json::to_string_pretty(&original).unwrap());

        let outcome = cleanup_legacy_providers(&path).unwrap();
        let LegacyCleanup::Removed { providers, backup } = outcome else {
            panic!("expected removal, got {outcome:?}");
        };
        assert_eq!(providers, ["edgee", "edgee-anthropic"]);
        assert_eq!(read_json(&backup), original);
        let after = read_json(&path);
        assert_eq!(after["providers"].as_object().unwrap().len(), 1);
        assert_eq!(after["providers"]["ollama"], original["providers"]["ollama"]);
        assert_eq!(after["other"], original["other"]);
    }

    #[test]
    fn keeps_the_first_backup_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let original = serde_json::json!({ "providers": { "edgee": cli_block("openai-completions") } });
        write(&path, &original.to_string());
        let backup = backup_path(&path);

        cleanup_legacy_providers(&path).unwrap();
        // An older CLI (or the user) recreates a block, then a later run cleans it again.
        write(
            &path,
            &serde_json::json!({ "providers": { "edgee": cli_block("openai-completions"), "new": {} } })
                .to_string(),
        );
        cleanup_legacy_providers(&path).unwrap();
        assert_eq!(read_json(&backup), original);
    }

    #[test]
    fn leaves_customised_blocks_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let mut custom = cli_block("openai-completions");
        custom["apiKey"] = Value::String("sk-literal".into());
        let content = serde_json::json!({ "providers": { "edgee": custom } }).to_string();
        write(&path, &content);

        let outcome = cleanup_legacy_providers(&path).unwrap();
        assert!(matches!(outcome, LegacyCleanup::Skipped(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn removes_only_the_generated_block_when_mixed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let mut custom = cli_block("anthropic-messages");
        custom["headers"]["x-edgee-session-id"] = Value::String("fixed".into());
        write(
            &path,
            &serde_json::json!({
                "providers": { "edgee": cli_block("openai-completions"), "edgee-anthropic": custom }
            })
            .to_string(),
        );

        let outcome = cleanup_legacy_providers(&path).unwrap();
        assert!(matches!(&outcome, LegacyCleanup::Removed { providers, .. } if providers == &["edgee"]));
        let after = read_json(&path);
        assert!(after["providers"].get("edgee").is_none());
        assert!(after["providers"].get("edgee-anthropic").is_some());
    }

    #[test]
    fn leaves_unparseable_and_comment_bearing_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        for content in ["{not json", "// note\n{\"providers\":{\"edgee\":{}}}", "[]"] {
            write(&path, content);
            let outcome = cleanup_legacy_providers(&path).unwrap();
            assert!(
                matches!(outcome, LegacyCleanup::Skipped(_) | LegacyCleanup::Nothing),
                "{content}: {outcome:?}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        }
    }

    #[test]
    fn missing_file_or_unrelated_providers_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        assert_eq!(cleanup_legacy_providers(&path).unwrap(), LegacyCleanup::Nothing);

        let content = r#"{"providers":{"ollama":{}}}"#;
        write(&path, content);
        assert_eq!(cleanup_legacy_providers(&path).unwrap(), LegacyCleanup::Nothing);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    }

    fn sample_context(debug: bool) -> LaunchContext {
        LaunchContext {
            version: CONTRACT_VERSION,
            session_id: "42999158-ae9f-5b44-8834-27675aacf427".into(),
            api_key: "ek_test".into(),
            api_key_id: None,
            user_token: "tok_test".into(),
            org_id: "org-1".into(),
            org_slug: "acme".into(),
            org_name: Some("Acme".into()),
            gateway_url: "https://api.edgee.ai".into(),
            console_url: "https://www.edgee.ai".into(),
            console_api_url: "https://api.edgee.app".into(),
            mcp_url: "https://api.edgee.app/mcp".into(),
            mcp_disabled: false,
            debug_headers: debug.then(|| DebugHeaders {
                pubkey: "pk".into(),
                salt: "salt".into(),
            }),
        }
    }

    #[test]
    fn context_uses_the_camel_case_wire_format() {
        let json = serde_json::to_value(sample_context(true)).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["sessionId"], "42999158-ae9f-5b44-8834-27675aacf427");
        assert_eq!(json["apiKey"], "ek_test");
        assert_eq!(json["userToken"], "tok_test");
        assert_eq!(json["consoleApiUrl"], "https://api.edgee.app");
        assert_eq!(json["mcpDisabled"], false);
        assert_eq!(json["debugHeaders"]["pubkey"], "pk");
    }

    #[test]
    fn context_omits_unset_optionals() {
        let json = serde_json::to_value(sample_context(false)).unwrap();
        assert!(json.get("debugHeaders").is_none());
        assert!(json.get("apiKeyId").is_none());
    }
}
