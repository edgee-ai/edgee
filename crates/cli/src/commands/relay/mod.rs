//! `edgee relay` — a local MITM proxy that logs LLM API traffic and reroutes
//! inference requests through the Edgee gateway.
//!
//! Terminates TLS with a locally-generated CA so HTTPS headers and bodies are
//! visible — but only for known LLM inference hosts; all other HTTPS is tunneled
//! without decryption (no leaf cert minted). Requests to inference paths
//! (`/v1/messages`, `/v1/responses`, `/v1/chat/completions`) are rewritten to the
//! Edgee gateway (with `x-edgee-*` auth injected); everything else is forwarded
//! to its original upstream. All matching traffic is logged.

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
const TARGETS: &[&str] = &["claude", "codex", "copilot-vscode", "cursor"];

/// Map a user-supplied agent name (including legacy aliases) to a canonical
/// launch/relay target. Returns `None` for unknown names.
fn canonicalize_target(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("claude"),
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

/// True for GUI editors relayed as passthrough providers (VS Code Copilot,
/// Cursor). These launch their own binary, leave the terminal free (so we
/// announce per-request), and the gateway forwards to the editor's own backend
/// rather than routing through an Edgee provider pipeline.
fn is_gui_editor(agent: &str) -> bool {
    is_copilot_vscode(agent) || is_cursor(agent)
}

/// Edgee credentials / console provider key for a **canonical** launch target.
/// Today most targets map 1:1; surfaces of the same product share a key
/// (e.g. `copilot-vscode` → `copilot`).
fn key_provider(target: &str) -> &str {
    match target {
        "copilot-vscode" => "copilot",
        // Future: "claude-desktop" | "claude-vscode" => "claude",
        // Future: "codex-desktop" => "codex",
        // Future: "copilot" (CLI) => "copilot",
        other => other,
    }
}

setup_command! {
    /// Launch/relay target (claude|codex|copilot-vscode|cursor). Aliases for
    /// Copilot-in-VS-Code: vscode-copilot|vscode|code. Launched unless
    /// --no-launch. Omit to run proxy-only with the claude key.
    pub agent: Option<String>,
    /// Don't spawn the agent; just run the proxy (for external clients, e.g. Claude Desktop).
    #[arg(long)]
    pub no_launch: bool,
    /// Port the proxy listens on. Defaults per agent (claude 41100, codex 41200)
    /// so `relay claude` and `relay codex` can run side by side.
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
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key(&provider).await?;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded(&provider).await?;
    }
    // VS Code can host Claude Code alongside Copilot chat. Provision the claude key
    // too so Claude's `/v1/messages` traffic reroutes through the claude pipeline.
    if is_copilot_vscode(&agent) {
        let reprov = crate::commands::auth::login::ensure_valid_provider_key("claude").await?;
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

    let (cert_pem, key_pem, cert_path) = ensure_ca()?;
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

    let handler = RelayHandler::new(sink, Arc::new(gateway.clone()), log_enabled);

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

    // Spawn the agent only when one is named; otherwise run proxy-only.
    match opts.agent.clone() {
        None => {
            print_external_help(&addr, &cert_path);
            proxy.start().await.context("relay proxy error")?;
        }
        Some(agent) => {
            if is_gui_editor(&agent) {
                print_gui_editor_hint(&agent);
            }
            let task = tokio::spawn(async move {
                let _ = proxy.start().await;
            });
            let status = run_agent(&agent, port, &cert_path, &session_id).await?;
            task.abort();
            if let Some(code) = status.code() {
                std::process::exit(code);
            }
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
        _ => 41100, // claude / copilot / proxy-only
    }
}

/// The Edgee key for `provider` from the active profile, if present.
fn provider_api_key(creds: &crate::config::Credentials, provider: &str) -> Option<String> {
    let p = match provider {
        "claude" => creds.claude.as_ref(),
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

/// Load the persisted CA, generating it on first use.
fn ensure_ca() -> Result<(String, String, PathBuf)> {
    let dir = crate::config::relay_ca_dir();
    let cert_path = dir.join("edgee-ca.pem");
    let key_path = dir.join("edgee-ca.key");

    if cert_path.exists() && key_path.exists() {
        let cert = std::fs::read_to_string(&cert_path)
            .with_context(|| format!("reading CA cert {}", cert_path.display()))?;
        let key = std::fs::read_to_string(&key_path)
            .with_context(|| format!("reading CA key {}", key_path.display()))?;
        return Ok((cert, key, cert_path));
    }

    std::fs::create_dir_all(&dir).with_context(|| format!("creating CA dir {}", dir.display()))?;
    let (cert_pem, key_pem) = generate_ca()?;
    std::fs::write(&cert_path, &cert_pem)
        .with_context(|| format!("writing CA cert {}", cert_path.display()))?;
    std::fs::write(&key_path, &key_pem)
        .with_context(|| format!("writing CA key {}", key_path.display()))?;
    Ok((cert_pem, key_pem, cert_path))
}

/// Generate a self-signed CA suitable for signing leaf certs at runtime.
fn generate_ca() -> Result<(String, String)> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
        KeyUsagePurpose,
    };

    let mut params =
        CertificateParams::new(Vec::new()).context("building CA certificate params")?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::CrlSign,
    ];
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Edgee CA");
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
    // GUI editors launch their own binary (VS Code Copilot → `code`, Cursor →
    // `cursor`). `--wait` keeps this process alive (and the proxy with it) until the
    // editor window is closed, instead of the launcher forking and returning at once.
    let (bin_name, args): (&str, &[&str]) = if is_copilot_vscode(agent) {
        ("code", &["--wait"])
    } else if is_cursor(agent) {
        ("cursor", &["--wait"])
    } else {
        (agent, &[])
    };
    let bin = crate::commands::launch::util::resolve_binary(bin_name);
    let proxy_url = format!("http://127.0.0.1:{port}");

    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args);
    // Cursor's AI inference (BidiAppend / RunSSE) goes through a Node `http2`
    // transport in the `cursor-always-local` extension, which honors HTTPS_PROXY
    // (set below) and NODE_EXTRA_CA_CERTS — so pointing that env at the relay is
    // enough to intercept inference. It speaks HTTP/2 by default, which the relay
    // can't MITM, so `ensure_cursor_http1` writes `cursor.general.disableHttp2`
    // (Settings → Network → HTTP Compatibility Mode → HTTP/1.1) to downgrade it.
    //
    // We deliberately do NOT set Chromium's `--proxy-server`: it blanket-routes
    // ALL of Cursor's HTTPS (GitHub, marketplace, sign-in, telemetry) through the
    // relay, which then MITMs/tunnels hosts we don't handle and breaks them (most
    // visibly, GitHub access). Only the inference URLs should hit the relay;
    // everything else keeps Cursor's normal, un-proxied, un-MITM'd path.
    if is_cursor(agent) {
        ensure_cursor_http1();
    }
    cmd.env("HTTPS_PROXY", &proxy_url);
    cmd.env("HTTP_PROXY", &proxy_url);
    cmd.env("https_proxy", &proxy_url);
    cmd.env("http_proxy", &proxy_url);
    // Make each agent's TLS stack trust the relay CA without a system-store install:
    //  - Node agents (Claude Code) and VS Code / Copilot / Cursor read NODE_EXTRA_CA_CERTS.
    //  - Codex (Rust) reads CODEX_CA_CERTIFICATE / SSL_CERT_FILE for its own client;
    //    it does NOT read NODE_EXTRA_CA_CERTS.
    cmd.env("NODE_EXTRA_CA_CERTS", ca_path);
    cmd.env("CODEX_CA_CERTIFICATE", ca_path);
    cmd.env("EDGEE_SESSION_ID", session_id);

    cmd.status()
        .await
        .with_context(|| format!("failed to launch '{bin_name}'"))
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

fn print_gui_editor_hint(agent: &str) {
    let (app, launch, feature) = if is_cursor(agent) {
        ("Cursor", "cursor --wait", "Cursor AI")
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
    }

    #[test]
    fn rejects_url_without_host() {
        // A path-only URI has no authority → reroute target can't be built.
        assert!(
            build_gateway_target("/no/host", "k".into(), "s".into(), None, false, None).is_err()
        );
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

