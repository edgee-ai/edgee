//! `edgee relay` — a local MITM proxy that logs LLM API traffic and reroutes
//! inference requests through the Edgee gateway.
//!
//! Only CONNECT tunnels to known LLM hosts are MITM-decrypted (with a locally-
//! generated CA) so HTTPS headers and bodies are visible; every other host is
//! blind-tunneled and never decrypted. On the decrypted hosts, requests to
//! inference paths (`/v1/messages`, `/v1/responses`, `/v1/chat/completions`) are
//! rewritten to the Edgee gateway (with `x-edgee-*` auth injected); other paths
//! are forwarded to their original upstream. All decrypted traffic is logged.

mod handler;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use console::style;
use http::uri::{Authority, Scheme};
use http::Uri;
use hudsucker::certificate_authority::RcgenAuthority;
use hudsucker::rustls::crypto::aws_lc_rs;
use hudsucker::Proxy;

use handler::{GatewayTarget, RelayHandler, Sink};

/// Canonical relay targets (same public names as `edgee launch`). See
/// `crates/cli/src/commands/launch/README.md` for naming rules.
///
/// Note: bare `copilot` is reserved for the future Copilot CLI launch target
/// and is intentionally not a relay alias here.
const TARGETS: &[&str] = &["claude", "claude-desktop", "codex", "copilot-vscode", "cursor"];

/// Map a user-supplied agent name (including legacy aliases) to a canonical
/// launch/relay target. Returns `None` for unknown names.
fn canonicalize_target(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("claude"),
        // Claude Desktop — its own surface of Claude, relayed like the GUI editors.
        "claude-desktop" | "claude_desktop" => Some("claude-desktop"),
        "codex" => Some("codex"),
        "cursor" => Some("cursor"),
        // GitHub Copilot in VS Code — canonical name is `copilot-vscode`.
        // `copilot` is reserved for the future Copilot CLI (not an alias here).
        "copilot-vscode" | "vscode-copilot" | "vscode" | "code" => Some("copilot-vscode"),
        _ => None,
    }
}

/// True for the GitHub Copilot (VS Code) relay target (launches the `code` binary).
fn is_copilot_vscode(agent: &str) -> bool {
    agent == "copilot-vscode"
}

/// True for the Cursor relay target (launches the `cursor` binary).
fn is_cursor(agent: &str) -> bool {
    agent == "cursor"
}

/// True for the Claude Desktop relay target (launches the Claude app bundle).
fn is_claude_desktop(agent: &str) -> bool {
    agent == "claude-desktop"
}

/// True for GUI editors relayed as passthrough providers (VS Code Copilot,
/// Cursor). These launch their own binary, leave the terminal free (so we
/// announce per-request), and the gateway forwards to the editor's own backend
/// rather than routing through an Edgee provider pipeline.
fn is_gui_editor(agent: &str) -> bool {
    is_copilot_vscode(agent) || is_cursor(agent)
}

/// True for GUI apps we launch ourselves (the passthrough editors plus Claude
/// Desktop). These share launch ergonomics — we spawn the app's own binary and
/// leave the terminal free — but only [`is_gui_editor`] targets are passthrough.
/// Claude Desktop routes through the real Claude provider pipeline like the CLI.
fn is_gui_app(agent: &str) -> bool {
    is_gui_editor(agent) || is_claude_desktop(agent)
}

/// Edgee credentials / console provider key for a **canonical** launch target.
/// Today most targets map 1:1; surfaces of the same product share a key
/// (e.g. `copilot-vscode` → `copilot`).
fn key_provider(target: &str) -> &str {
    match target {
        "copilot-vscode" => "copilot",
        // Claude Desktop is a dedicated backend agent with its own key/compression
        // (coding_assistant `claude_desktop`), so it maps to its own provider slot
        // rather than sharing the `claude` (Claude Code) key.
        "claude-desktop" => "claude_desktop",
        // Future: "claude-vscode" => "claude",
        // Future: "codex-desktop" => "codex",
        // Future: "copilot" (CLI) => "copilot",
        other => other,
    }
}

setup_command! {
    /// Launch/relay target (claude|claude-desktop|codex|copilot-vscode|cursor).
    /// Aliases for Copilot-in-VS-Code: vscode-copilot|vscode|code. Launched unless
    /// --no-launch. Omit to run proxy-only with the claude key.
    pub agent: Option<String>,
    /// Don't spawn the agent; just run the proxy (for external clients, e.g. Claude Desktop).
    #[arg(long)]
    pub no_launch: bool,
    /// Port the proxy listens on. Defaults per agent (claude 41100, codex 41200,
    /// cursor 41300, claude-desktop 41400) so multiple relays can run side by side.
    #[arg(long)]
    pub port: Option<u16>,
    /// Write relayed-traffic logs to this file (appended). If unset, logging is off.
    #[arg(long)]
    pub log_output: Option<PathBuf>,
}

pub async fn run(opts: Options) -> Result<()> {
    let raw = opts.agent.clone().unwrap_or_else(|| "claude".to_string());
    let agent = canonicalize_target(&raw)
        .ok_or_else(|| anyhow::anyhow!("unknown agent '{raw}' (expected {})", TARGETS.join("|")))?
        .to_string();
    // The Edgee provider key backing the gateway reroute. GUI editors (VS Code
    // Copilot, Cursor) map to their own passthrough provider key.
    let provider = key_provider(&agent).to_string();

    // Auth bootstrap — same flow as `edgee launch`.
    let mut creds = crate::config::read()?;
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }
    crate::commands::auth::login::ensure_org_selected().await?;
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key(&provider)
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded(&provider).await?;
    }
    // VS Code can host Claude Code alongside Copilot chat. Provision the claude key
    // too so Claude's `/v1/messages` traffic reroutes through the claude pipeline.
    if is_copilot_vscode(&agent) {
        let reprov = crate::commands::auth::login::ensure_valid_provider_key("claude")
            .await?
            .created;
        if reprov {
            crate::commands::auth::login::ensure_onboarded("claude").await?;
        }
    }
    creds = crate::config::read()?;

    let api_key = provider_api_key(&creds, &provider).ok_or_else(|| {
        anyhow::anyhow!("no Edgee API key for '{provider}'; run `edgee auth login`")
    })?;
    // Only wired for the VS Code relay; None elsewhere so `/v1/messages` keeps using
    // the relay's own key.
    let claude_api_key = if is_copilot_vscode(&agent) {
        provider_api_key(&creds, "claude")
    } else {
        None
    };
    let session_id = uuid::Uuid::new_v4().to_string();
    let repo = crate::git::detect_origin();

    let gateway_url = crate::commands::launch::resolve_gateway_base_url(&creds).await;
    // GUI editors have no Edgee provider pipeline; the gateway forwards their
    // rerouted calls to the editor's own backend, so record the original upstream.
    let passthrough_to_upstream = is_gui_editor(&agent);
    let gateway = build_gateway_target(
        &gateway_url,
        api_key,
        session_id.clone(),
        repo,
        passthrough_to_upstream,
        claude_api_key,
    )?;

    // claude-desktop uses its own name-constrained CA (it's the only target trusted
    // in a system keychain); every other target uses the shared unconstrained CA.
    let (cert_pem, key_pem, cert_path) = if is_claude_desktop(&agent) {
        ensure_claude_desktop_ca()?
    } else {
        ensure_ca()?
    };
    let ca = build_ca(&cert_pem, &key_pem)?;
    let port = opts.port.unwrap_or_else(|| default_port(&provider));
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    // Logging is opt-in: enabled only when a log file is given.
    let log_enabled = opts.log_output.is_some();
    let sink = match &opts.log_output {
        Some(path) => {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("opening log file {}", path.display()))?;
            Sink::file(f)
        }
        // Placeholder sink; never written to when logging is disabled.
        None => Sink::stdout(),
    };

    // Only the Copilot-VS-Code relay needs GitHub's control-plane host
    // (api.github.com) MITM'd for token/model discovery; other relays blind-tunnel
    // it so their MCP servers can reach GitHub with GitHub's real certificate.
    let handler = RelayHandler::new(
        sink,
        Arc::new(gateway.clone()),
        log_enabled,
        is_copilot_vscode(&agent),
    );

    let proxy = Proxy::builder()
        .with_addr(addr)
        .with_ca(ca)
        .with_rustls_connector(aws_lc_rs::default_provider())
        .with_http_handler(handler)
        .with_graceful_shutdown(shutdown_signal())
        .build()
        .context("failed to build relay proxy")?;

    print_banner(
        &addr,
        &cert_path,
        opts.log_output.as_deref(),
        &gateway,
        &session_id,
    );

    // Spawn the agent only when one is named; otherwise run proxy-only. Launch
    // uses the canonical `agent` (e.g. `vscode` → `copilot-vscode`), not the raw
    // user input, so GUI-editor detection and binary resolution work.
    if opts.agent.is_none() {
        print_external_help(&addr, &cert_path);
        proxy.start().await.context("relay proxy error")?;
    } else {
        if is_gui_app(&agent) {
            print_gui_app_hint(&agent);
        }
        // Claude Desktop (Chromium) verifies API certs against the macOS system
        // trust store — it ignores NODE_EXTRA_CA_CERTS and the --ignore-certificate-*
        // switches. Trust our CA there for the lifetime of this session only, then
        // remove it on exit, so the machine-wide MITM root never outlives the relay.
        // `SessionTrust`'s Drop and the Ctrl-C branch below both tear it down; a hard
        // kill can't run cleanup, so install also purges any stale copy first.
        let _session_trust = if is_claude_desktop(&agent) {
            Some(SessionTrust::install(&cert_path)?)
        } else {
            None
        };
        let task = tokio::spawn(async move {
            let _ = proxy.start().await;
        });
        let status = tokio::select! {
            r = run_agent(&agent, port, &cert_path, &session_id) => r?,
            _ = tokio::signal::ctrl_c() => {
                // Explicit teardown before exit: `process::exit` skips Drop, so the
                // SessionTrust guard would not otherwise remove the cert.
                drop(_session_trust);
                std::process::exit(130);
            }
        };
        task.abort();
        // The app exited on its own. `process::exit` below also skips Drop, so
        // remove the session trust explicitly before leaving.
        drop(_session_trust);
        if let Some(code) = status.code() {
            std::process::exit(code);
        }
    }

    Ok(())
}

/// Run the relay for `agent` with default options. Entry point for
/// `edgee launch <agent> --relay`.
pub async fn run_for_agent(agent: &str) -> Result<()> {
    run(Options {
        agent: Some(agent.to_string()),
        no_launch: false,
        port: None,
        log_output: None,
    })
    .await
}

/// Default listen port per agent, picked from an uncommon range so two relays
/// (`relay claude` + `relay codex`) don't collide out of the box.
fn default_port(provider: &str) -> u16 {
    match provider {
        "codex" => 41200,
        "cursor" => 41300,
        "claude_desktop" => 41400,
        _ => 41100, // claude / copilot / proxy-only
    }
}

/// The Edgee key for `provider` from the active profile, if present.
fn provider_api_key(creds: &crate::config::Credentials, provider: &str) -> Option<String> {
    let p = match provider {
        "claude" => creds.claude.as_ref(),
        "claude_desktop" => creds.claude_desktop.as_ref(),
        "codex" => creds.codex.as_ref(),
        "opencode" => creds.opencode.as_ref(),
        "crush" => creds.crush.as_ref(),
        "copilot" => creds.copilot.as_ref(),
        "cursor" => creds.cursor.as_ref(),
        _ => None,
    }?;
    if p.api_key.is_empty() {
        None
    } else {
        Some(p.api_key.clone())
    }
}

/// Parse the resolved gateway URL into a reroute target.
fn build_gateway_target(
    url: &str,
    api_key: String,
    session_id: String,
    repo: Option<String>,
    passthrough_to_upstream: bool,
    claude_api_key: Option<String>,
) -> Result<GatewayTarget> {
    let uri: Uri = url.parse().with_context(|| format!("parsing gateway url {url}"))?;
    let scheme = uri.scheme().cloned().unwrap_or(Scheme::HTTPS);
    let authority: Authority = uri
        .authority()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("gateway url has no host: {url}"))?;
    let base_path = uri.path().trim_end_matches('/').to_string();
    Ok(GatewayTarget {
        scheme,
        authority,
        base_path,
        api_key,
        session_id,
        repo,
        passthrough_to_upstream,
        claude_api_key,
    })
}

/// Common Name of the dedicated, name-constrained CA used only for the
/// `claude-desktop` relay. Kept distinct from the shared `Edgee CA` so
/// [`SessionTrust`] can add/remove exactly this cert in the keychain, and so it
/// never gets trusted for anything but Anthropic.
const CLAUDE_DESKTOP_CA_CN: &str = "Edgee Claude Desktop CA";

/// The shared, unconstrained relay CA (used by every target except claude-desktop
/// via per-process `NODE_EXTRA_CA_CERTS`, never installed in a system trust store).
fn ensure_ca() -> Result<(String, String, PathBuf)> {
    ensure_ca_named("edgee-ca", "Edgee CA", &[])
}

/// The dedicated CA for `claude-desktop`. Because this one gets **trusted in the
/// macOS system keychain** (Chromium reads only the OS store), it's name-constrained
/// to `anthropic.com` — Claude Desktop's only MITM'd host — so that even while
/// trusted (or if its key leaked) it can't vouch for any other domain. Chromium
/// enforces X.509 name constraints on locally-trusted roots.
fn ensure_claude_desktop_ca() -> Result<(String, String, PathBuf)> {
    ensure_ca_named(
        "edgee-claude-desktop-ca",
        CLAUDE_DESKTOP_CA_CN,
        &["anthropic.com"],
    )
}

/// Load the persisted CA at `<ca dir>/<file_stem>.{pem,key}`, generating it on
/// first use with the given Common Name and DNS name-constraints (empty = none).
fn ensure_ca_named(
    file_stem: &str,
    common_name: &str,
    permitted_dns: &[&str],
) -> Result<(String, String, PathBuf)> {
    let dir = crate::config::relay_ca_dir();
    let cert_path = dir.join(format!("{file_stem}.pem"));
    let key_path = dir.join(format!("{file_stem}.key"));

    if cert_path.exists() && key_path.exists() {
        let cert = std::fs::read_to_string(&cert_path)
            .with_context(|| format!("reading CA cert {}", cert_path.display()))?;
        let key = std::fs::read_to_string(&key_path)
            .with_context(|| format!("reading CA key {}", key_path.display()))?;
        return Ok((cert, key, cert_path));
    }

    std::fs::create_dir_all(&dir).with_context(|| format!("creating CA dir {}", dir.display()))?;
    let (cert_pem, key_pem) = generate_ca(common_name, permitted_dns)?;
    std::fs::write(&cert_path, &cert_pem)
        .with_context(|| format!("writing CA cert {}", cert_path.display()))?;
    std::fs::write(&key_path, &key_pem)
        .with_context(|| format!("writing CA key {}", key_path.display()))?;
    Ok((cert_pem, key_pem, cert_path))
}

/// Generate a self-signed CA suitable for signing leaf certs at runtime. When
/// `permitted_dns` is non-empty the CA carries an X.509 name-constraints extension
/// limiting the DNS names it may issue certificates for (RFC 5280); a constraint of
/// `anthropic.com` also permits `api.anthropic.com` and other subdomains.
fn generate_ca(common_name: &str, permitted_dns: &[&str]) -> Result<(String, String)> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, GeneralSubtree, IsCa,
        KeyPair, KeyUsagePurpose, NameConstraints,
    };

    let mut params =
        CertificateParams::new(Vec::new()).context("building CA certificate params")?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::CrlSign,
    ];
    if !permitted_dns.is_empty() {
        params.name_constraints = Some(NameConstraints {
            permitted_subtrees: permitted_dns
                .iter()
                .map(|d| GeneralSubtree::DnsName(d.to_string()))
                .collect(),
            excluded_subtrees: Vec::new(),
        });
    }
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    dn.push(DnType::OrganizationName, "Edgee");
    params.distinguished_name = dn;

    let key_pair = KeyPair::generate().context("generating CA key pair")?;
    let cert = params.self_signed(&key_pair).context("self-signing CA")?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}

/// Build a hudsucker authority from PEM material.
fn build_ca(cert_pem: &str, key_pem: &str) -> Result<RcgenAuthority> {
    use rcgen::{Issuer, KeyPair};

    let key_pair = KeyPair::from_pem(key_pem).context("parsing CA key")?;
    let issuer = Issuer::from_ca_cert_pem(cert_pem, key_pair).context("parsing CA cert")?;
    Ok(RcgenAuthority::new(
        issuer,
        1_000,
        aws_lc_rs::default_provider(),
    ))
}

/// Session-scoped trust of the relay CA in the macOS system keychain. Claude
/// Desktop (Chromium) only trusts the OS certificate store, so MITM'ing it means
/// installing our CA as a trusted root — a machine-wide grant. To keep that grant
/// from outliving the relay, [`install`](Self::install) adds it on launch and
/// [`Drop`] removes it on exit, bounding the window to this session.
///
/// Adding an admin-domain (System keychain) trust root needs privilege, so this
/// shells out to `sudo security` (prompts once; the purge + add + remove reuse the
/// cached credential). A hard kill can't run `Drop`, so `install` first purges any
/// stale copy left by a crashed prior run. On non-macOS this is a no-op.
struct SessionTrust;

impl SessionTrust {
    fn install(ca_path: &Path) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            // Idempotent: clear any leftover from a prior (possibly crashed) session
            // before adding a fresh trust root.
            Self::remove_cert();
            eprintln!(
                "{}",
                style("Trusting Edgee CA in the system keychain for this session (admin required)…")
                    .dim()
            );
            let status = std::process::Command::new("sudo")
                .args([
                    "security",
                    "add-trusted-cert",
                    "-d",
                    "-r",
                    "trustRoot",
                    "-k",
                    "/Library/Keychains/System.keychain",
                ])
                .arg(ca_path)
                .status()
                .context("running `sudo security add-trusted-cert`")?;
            if !status.success() {
                anyhow::bail!(
                    "failed to trust the Edgee CA in the system keychain \
                     (Claude Desktop needs it to accept the relay). Left nothing installed."
                );
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = ca_path;
        }
        Ok(SessionTrust)
    }

    #[cfg(target_os = "macos")]
    fn remove_cert() {
        // Best-effort; a missing cert (nothing to remove) is fine. Targets the
        // dedicated, name-constrained CA by CN — never the shared `Edgee CA`.
        let _ = std::process::Command::new("sudo")
            .args([
                "security",
                "delete-certificate",
                "-c",
                CLAUDE_DESKTOP_CA_CN,
                "/Library/Keychains/System.keychain",
            ])
            .status();
    }
}

impl Drop for SessionTrust {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            eprintln!(
                "{}",
                style("Removing Edgee CA trust from the system keychain…").dim()
            );
            Self::remove_cert();
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Path to Cursor's user `settings.json`, per platform:
///   Linux:   `$XDG_CONFIG_HOME/Cursor/User/settings.json` (else `~/.config/...`)
///   macOS:   `~/Library/Application Support/Cursor/User/settings.json`
///   Windows: `%APPDATA%\Cursor\User\settings.json`
fn cursor_settings_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| h.join("Library/Application Support/Cursor/User/settings.json"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Cursor/User/settings.json"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".config")))
            .map(|c| c.join("Cursor/User/settings.json"))
    }
}

/// Ensure Cursor's `settings.json` has `cursor.general.disableHttp2: true` so its
/// AI transport speaks HTTP/1.1 (which the relay can MITM) rather than HTTP/2.
/// Merges the key into any existing settings, preserving other entries, and
/// creates the file if absent. Best-effort: on any failure it prints a hint to
/// set it manually rather than aborting the launch, and it never clobbers a file
/// it can't parse (e.g. one with `//` comments).
fn ensure_cursor_http1() {
    const KEY: &str = "cursor.general.disableHttp2";
    let manual = || {
        eprintln!(
            "{}",
            style(format!(
                "  Could not update Cursor settings automatically — set \
                 \"{KEY}\": true (Settings → Network → HTTP Compatibility Mode → \
                 HTTP/1.1) so relay traffic is intercepted."
            ))
            .dim()
        );
    };

    let Some(path) = cursor_settings_path() else {
        manual();
        return;
    };

    let current = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            manual();
            return;
        }
    };

    // `None` => already set (nothing to write); `Err` => unparseable, leave as-is.
    let body = match cursor_settings_with_http1(&current) {
        Ok(Some(body)) => body,
        Ok(None) => return,
        Err(()) => {
            manual();
            return;
        }
    };

    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &body)
    };
    if write().is_err() {
        manual();
    }
}

/// Merge `cursor.general.disableHttp2: true` into Cursor's `settings.json` body.
/// `current` is the existing file contents (empty string when absent). Returns
/// `Ok(Some(new_body))` to write, `Ok(None)` when it's already set (no write
/// needed), or `Err(())` when `current` isn't a JSON object (don't clobber it).
fn cursor_settings_with_http1(current: &str) -> Result<Option<String>, ()> {
    const KEY: &str = "cursor.general.disableHttp2";
    let mut obj = if current.trim().is_empty() {
        serde_json::Map::new()
    } else {
        match serde_json::from_str::<serde_json::Value>(current) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => return Err(()),
        }
    };
    if obj.get(KEY) == Some(&serde_json::Value::Bool(true)) {
        return Ok(None);
    }
    obj.insert(KEY.to_string(), serde_json::Value::Bool(true));
    let mut body = serde_json::to_string_pretty(&serde_json::Value::Object(obj)).map_err(|_| ())?;
    body.push('\n');
    Ok(Some(body))
}

/// Spawn the named agent wired through the proxy. The proxy injects Edgee auth on
/// reroute, so no base-URL / custom-header env is needed here.
async fn run_agent(
    agent: &str,
    port: u16,
    ca_path: &Path,
    session_id: &str,
) -> Result<std::process::ExitStatus> {
    let proxy_url = format!("http://127.0.0.1:{port}");

    // Claude Desktop is a GUI Electron app with no CLI shim, so we spawn the app
    // bundle's own binary directly (not `open`/a wrapper) — that keeps this process
    // attached until the app quits (holding the proxy open) and lets the proxy env
    // propagate into it. Like Cursor, its Electron net stack won't honor
    // HTTPS_PROXY, so route it explicitly with Chromium's --proxy-server. Its API
    // calls verify against the macOS system trust store (Chromium net ignores
    // NODE_EXTRA_CA_CERTS *and* the --ignore-certificate-errors* switches when
    // passed via argv), so the relay CA must be trusted in the keychain — handled
    // for the session's lifetime by `SessionTrust` in `run`.
    let mut cmd = if is_claude_desktop(agent) {
        let bin = claude_desktop_binary()?;
        let mut c = tokio::process::Command::new(bin);
        c.arg(format!("--proxy-server={proxy_url}"));
        c
    } else {
        // GUI editors launch their own binary (VS Code Copilot → `code`, Cursor →
        // `cursor`). `--wait` keeps this process alive (and the proxy with it) until
        // the editor window is closed, instead of the launcher forking and returning
        // at once.
        let (bin_name, args): (&str, &[&str]) = if is_copilot_vscode(agent) {
            ("code", &["--wait"])
        } else if is_cursor(agent) {
            ("cursor", &["--wait"])
        } else {
            (agent, &[])
        };
        let bin = crate::commands::launch::util::resolve_binary(bin_name);
        let mut c = tokio::process::Command::new(bin);
        c.args(args);
        // Cursor's Electron net module ignores HTTPS_PROXY; --proxy-server routes
        // all HTTPS traffic through the relay so BidiAppend / RunSSE are intercepted.
        // NB: Cursor's AI calls don't use Chromium's net stack — they go through a
        // Node `http2` transport in the `cursor-always-local` extension, which reads
        // NODE_EXTRA_CA_CERTS (set below) but speaks HTTP/2 by default, which the relay
        // can't MITM. `ensure_cursor_http1` writes `cursor.general.disableHttp2` (the
        // Settings → Network → HTTP Compatibility Mode → HTTP/1.1 toggle) so the
        // transport downgrades to HTTP/1.1 and the relay can see it.
        if is_cursor(agent) {
            c.arg(format!("--proxy-server={proxy_url}"));
            ensure_cursor_http1();
        }
        c
    };
    cmd.env("HTTPS_PROXY", &proxy_url);
    cmd.env("HTTP_PROXY", &proxy_url);
    cmd.env("https_proxy", &proxy_url);
    cmd.env("http_proxy", &proxy_url);
    // Exempt loopback from the proxy. The proxy env is inherited by every child
    // process the agent spawns — notably MCP servers, which commonly talk to a
    // local endpoint (`http://127.0.0.1:PORT`). Without a bypass, Node-based MCP
    // transports honor HTTP_PROXY and route those loopback calls back through the
    // relay (which can't forward arbitrary loopback plain-HTTP), so the MCP fails
    // to connect. Chromium's own `--proxy-server` already bypasses loopback; this
    // covers the env-var path the subprocesses use.
    let no_proxy = build_no_proxy();
    cmd.env("NO_PROXY", &no_proxy);
    cmd.env("no_proxy", &no_proxy);
    // Make each agent's TLS stack trust the relay CA without a system-store install:
    //  - Node agents (Claude Code) and VS Code / Copilot / Cursor read NODE_EXTRA_CA_CERTS.
    //  - Codex (Rust) reads CODEX_CA_CERTIFICATE / SSL_CERT_FILE for its own client;
    //    it does NOT read NODE_EXTRA_CA_CERTS.
    cmd.env("NODE_EXTRA_CA_CERTS", ca_path);
    cmd.env("CODEX_CA_CERTIFICATE", ca_path);
    cmd.env("EDGEE_SESSION_ID", session_id);

    cmd.status()
        .await
        .with_context(|| format!("failed to launch '{agent}'"))
}

/// Resolve the Claude Desktop executable to launch behind the relay. Claude
/// Desktop ships on macOS and Windows; we spawn the app's own binary (not
/// `open`/a shim) so the relay's proxy env and CA trust propagate to it.
fn claude_desktop_binary() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut candidates = vec![PathBuf::from("/Applications/Claude.app/Contents/MacOS/Claude")];
        if let Some(home) = home_dir() {
            candidates.push(home.join("Applications/Claude.app/Contents/MacOS/Claude"));
        }
        candidates.into_iter().find(|p| p.exists()).ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Desktop not found. Install it from https://claude.ai/download \
                 (looked in /Applications and ~/Applications)."
            )
        })
    }
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            std::env::var_os("LOCALAPPDATA")
                .map(|a| PathBuf::from(a).join("AnthropicClaude").join("claude.exe")),
            std::env::var_os("PROGRAMFILES")
                .map(|a| PathBuf::from(a).join("Claude").join("claude.exe")),
        ];
        candidates
            .into_iter()
            .flatten()
            .find(|p| p.exists())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Claude Desktop not found. Install it from https://claude.ai/download."
                )
            })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        anyhow::bail!("Claude Desktop is not available on this platform (macOS/Windows only).")
    }
}

/// The proxy-bypass list for relayed agents: loopback (so local MCP servers and
/// other localhost services connect directly) plus any `NO_PROXY`/`no_proxy` the
/// user already had in the environment, deduplicated and order-preserving.
fn build_no_proxy() -> String {
    const LOOPBACK: &[&str] = &["localhost", "127.0.0.1", "::1"];
    let inherited = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();

    let mut entries: Vec<String> = LOOPBACK.iter().map(|s| s.to_string()).collect();
    for part in inherited.split(',') {
        let part = part.trim();
        if !part.is_empty() && !entries.iter().any(|e| e.eq_ignore_ascii_case(part)) {
            entries.push(part.to_string());
        }
    }
    entries.join(",")
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn print_banner(
    addr: &SocketAddr,
    cert_path: &Path,
    log_output: Option<&Path>,
    gateway: &GatewayTarget,
    session_id: &str,
) {
    println!("{}", style("edgee relay").bold().green());
    println!("  proxy:    http://{addr}");
    println!("  CA cert:  {}", cert_path.display());
    println!(
        "  gateway:  {}://{}  (reroute /v1/messages, /v1/responses, /v1/chat/completions)",
        gateway.scheme, gateway.authority
    );
    println!("  session:  {session_id}");
    println!(
        "  console:  {}/sessions/{session_id}",
        crate::config::console_base_url()
    );
    match log_output {
        Some(p) => println!("  logs:     {}", p.display()),
        None => println!("  logs:     disabled"),
    }
    println!();
}

fn print_gui_app_hint(agent: &str) {
    let (app, launch, feature) = if is_cursor(agent) {
        ("Cursor", "cursor --wait", "Cursor AI")
    } else if is_claude_desktop(agent) {
        ("Claude Desktop", "the Claude app", "Claude Desktop")
    } else {
        ("VS Code", "code --wait", "Copilot Chat")
    };
    println!(
        "{}",
        style(format!("Launching {app} ({launch}) behind the relay.")).bold()
    );
    println!(
        "  {}",
        style(format!(
            "Quit any running {app} first — the proxy env only applies to a freshly"
        ))
        .dim()
    );
    println!(
        "  {}",
        style(format!(
            "spawned instance. {feature} traffic then reroutes through the gateway."
        ))
        .dim()
    );
    println!();
}

fn print_external_help(addr: &SocketAddr, cert_path: &Path) {
    println!("{}", style("To relay an external process:").bold());
    println!("  export HTTPS_PROXY=http://{addr}");
    println!(
        "  export NODE_EXTRA_CA_CERTS={}   # Node/Claude Code",
        cert_path.display()
    );
    println!("  # GUI apps (Claude Desktop): trust the CA in the system keychain");
    println!();
    println!("{}", style("Ctrl-C to stop.").dim());
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_gateway() {
        let gw =
            build_gateway_target("https://edgee.io", "k".into(), "s".into(), None, false, None)
                .unwrap();
        assert_eq!(gw.scheme, Scheme::HTTPS);
        assert_eq!(gw.authority.as_str(), "edgee.io");
        assert_eq!(gw.base_path, "");
    }

    #[test]
    fn parses_local_override() {
        let gw =
            build_gateway_target("http://127.0.0.1:9999", "k".into(), "s".into(), None, false, None)
                .unwrap();
        assert_eq!(gw.scheme.as_str(), "http");
        assert_eq!(gw.authority.as_str(), "127.0.0.1:9999");
        assert_eq!(gw.base_path, "");
    }

    #[test]
    fn canonicalize_maps_copilot_vscode_aliases() {
        for a in ["copilot-vscode", "vscode-copilot", "vscode", "code"] {
            assert_eq!(canonicalize_target(a), Some("copilot-vscode"), "{a}");
        }
        // Bare `copilot` is reserved for the future CLI — not a VS Code alias.
        assert_eq!(canonicalize_target("copilot"), None);
        assert_eq!(canonicalize_target("claude"), Some("claude"));
        assert_eq!(canonicalize_target("codex"), Some("codex"));
        assert_eq!(canonicalize_target("cursor"), Some("cursor"));
        assert_eq!(canonicalize_target("unknown"), None);
    }

    #[test]
    fn canonicalize_maps_claude_desktop_aliases() {
        for a in ["claude-desktop", "claude_desktop"] {
            assert_eq!(canonicalize_target(a), Some("claude-desktop"), "{a}");
        }
    }

    #[test]
    fn claude_desktop_is_gui_app_not_passthrough_editor() {
        assert!(is_claude_desktop("claude-desktop"));
        // It's a GUI app we launch, but NOT a passthrough editor: it routes
        // through the real Claude provider pipeline, so passthrough stays off.
        assert!(is_gui_app("claude-desktop"));
        assert!(!is_gui_editor("claude-desktop"));
        assert!(!is_claude_desktop("cursor"));
        assert!(!is_claude_desktop("claude"));
    }

    #[test]
    fn claude_desktop_reroute_uses_claude_desktop_key() {
        assert_eq!(key_provider("claude-desktop"), "claude_desktop");
    }

    #[test]
    fn claude_desktop_ca_is_name_constrained() {
        // The claude-desktop CA gets system-keychain trust, so it must be limited to
        // Anthropic. The shared CA must stay unconstrained (it MITMs other providers).
        let (constrained, _) = generate_ca(CLAUDE_DESKTOP_CA_CN, &["anthropic.com"]).unwrap();
        assert!(constrained.contains("BEGIN CERTIFICATE"));
        let (shared, _) = generate_ca("Edgee CA", &[]).unwrap();
        // A name-constrained cert carries the extension, so its DER is meaningfully
        // larger; the unconstrained one omits it. Guards against dropping the arg.
        assert!(
            constrained.len() > shared.len(),
            "constrained CA should carry the name-constraints extension"
        );
    }


    #[test]
    fn copilot_vscode_agent_aliases_recognized() {
        for a in ["copilot-vscode", "vscode-copilot", "code", "vscode"] {
            let canon = canonicalize_target(a).unwrap();
            assert!(
                is_copilot_vscode(canon),
                "{a} should canonicalize to copilot-vscode"
            );
        }
        assert!(!is_copilot_vscode("claude"));
        assert!(!is_copilot_vscode("codex"));
        assert!(!is_copilot_vscode("copilot"));
    }

    #[test]
    fn copilot_vscode_reroute_uses_copilot_key() {
        for a in ["copilot-vscode", "vscode-copilot", "code", "vscode"] {
            let canon = canonicalize_target(a).unwrap();
            assert_eq!(key_provider(canon), "copilot");
        }
        // Real providers back their own key.
        assert_eq!(key_provider("claude"), "claude");
        assert_eq!(key_provider("codex"), "codex");
    }

    #[test]
    fn cursor_agent_recognized() {
        assert!(is_cursor("cursor"));
        assert!(!is_cursor("claude"));
        assert!(!is_cursor("code"));
        // Both Copilot-in-VS-Code and Cursor are GUI editors; TUI agents are not.
        assert!(is_gui_editor("cursor"));
        assert!(is_gui_editor("copilot-vscode"));
        assert!(!is_gui_editor("claude"));
        assert!(!is_gui_editor("codex"));
    }

    #[test]
    fn cursor_reroute_uses_cursor_key() {
        assert_eq!(key_provider("cursor"), "cursor");
    }

    #[test]
    fn default_ports_are_distinct_per_agent() {
        assert_eq!(default_port("claude"), 41100);
        assert_eq!(default_port("codex"), 41200);
        assert_eq!(default_port("cursor"), 41300);
        assert_eq!(default_port("claude_desktop"), 41400);
    }

    #[test]
    fn rejects_url_without_host() {
        // A path-only URI has no authority → reroute target can't be built.
        assert!(
            build_gateway_target("/no/host", "k".into(), "s".into(), None, false, None).is_err()
        );
    }

    #[test]
    fn no_proxy_includes_loopback() {
        let list = build_no_proxy();
        for h in ["localhost", "127.0.0.1", "::1"] {
            assert!(list.split(',').any(|e| e == h), "missing {h} in {list}");
        }
    }

    #[test]
    fn cursor_http1_creates_settings_when_empty() {
        let body = cursor_settings_with_http1("").unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["cursor.general.disableHttp2"], serde_json::json!(true));
        assert!(body.ends_with('\n'));
    }

    #[test]
    fn cursor_http1_merges_and_preserves_existing_keys() {
        let existing = r#"{"editor.fontSize": 14, "cursor.general.disableHttp2": false}"#;
        let body = cursor_settings_with_http1(existing).unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["cursor.general.disableHttp2"], serde_json::json!(true));
        assert_eq!(v["editor.fontSize"], serde_json::json!(14));
    }

    #[test]
    fn cursor_http1_noop_when_already_set() {
        let existing = r#"{"cursor.general.disableHttp2": true}"#;
        assert_eq!(cursor_settings_with_http1(existing), Ok(None));
    }

    #[test]
    fn cursor_http1_refuses_to_clobber_unparseable() {
        // A file with comments (valid JSONC, invalid JSON) must be left untouched.
        let jsonc = "{\n  // proxy tweak\n  \"editor.fontSize\": 14\n}";
        assert_eq!(cursor_settings_with_http1(jsonc), Err(()));
    }
}

