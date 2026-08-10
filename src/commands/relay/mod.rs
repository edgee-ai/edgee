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
/// `src/commands/launch/README.md` for naming rules.
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

/// Display name of the GUI editor behind a relay target, for user-facing messages.
/// Only meaningful for [`is_gui_editor`] targets.
fn editor_app_name(agent: &str) -> &'static str {
    if is_cursor(agent) {
        "Cursor"
    } else {
        "VS Code"
    }
}

/// How to put Cursor's `cursor` CLI on `PATH` from inside the editor. macOS-only:
/// the other platforms' packages ship the CLI on `PATH`, so the hint would be dead
/// code there.
#[cfg(target_os = "macos")]
const CURSOR_PATH_HINT: &[&str] = &[
    "Not working? `cursor` may not be on your PATH: open the Command Palette",
    "(Cmd+Shift+P) and run \"Install 'cursor' command\".",
];

/// How to put VS Code's `code` CLI on `PATH` from inside the editor. macOS-only,
/// for the same reason as [`CURSOR_PATH_HINT`].
#[cfg(target_os = "macos")]
const VSCODE_PATH_HINT: &[&str] = &[
    "Not working? `code` may not be on your PATH: open the Command Palette",
    "(Cmd+Shift+P), type \"shell command\", and run",
    "\"Shell Command: Install 'code' command in PATH\".",
];

/// macOS installs Cursor / VS Code as app bundles without their CLI on `PATH`
/// (the editors add it themselves from the Command Palette), so `cursor --wait`
/// / `code --wait` fails until the user runs that command. `None` elsewhere —
/// Linux and Windows packages ship the CLI on `PATH` already.
fn macos_cli_path_hint(agent: &str) -> Option<&'static [&'static str]> {
    #[cfg(target_os = "macos")]
    {
        if is_cursor(agent) {
            Some(CURSOR_PATH_HINT)
        } else if is_copilot_vscode(agent) {
            Some(VSCODE_PATH_HINT)
        } else {
            None
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = agent;
        None
    }
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
    /// Remove the Edgee Claude Desktop CA from the system keychain, then exit
    /// (macOS). Undoes the one-time trust `relay claude-desktop` installs.
    #[arg(long)]
    pub untrust: bool,
    /// Never prompt: fail instead of running interactive login / org selection /
    /// first-run onboarding. For GUI front-ends that drive the relay headlessly.
    #[arg(long)]
    pub non_interactive: bool,
}

pub async fn run(opts: Options) -> Result<()> {
    if opts.untrust {
        return untrust_ca();
    }
    let raw = opts.agent.clone().unwrap_or_else(|| "claude".to_string());
    let agent = canonicalize_target(&raw)
        .ok_or_else(|| anyhow::anyhow!("unknown agent '{raw}' (expected {})", TARGETS.join("|")))?
        .to_string();
    // The Edgee provider key backing the gateway reroute. GUI editors (VS Code
    // Copilot, Cursor) map to their own passthrough provider key.
    let provider = key_provider(&agent).to_string();

    // Auth bootstrap — same flow as `edgee launch`. When driven headlessly
    // (`--non-interactive`), never prompt: bail if the prerequisites aren't
    // already in place, and skip first-run onboarding (the key is still minted
    // with default compression).
    let interactive = !opts.non_interactive;
    let mut creds = crate::config::read()?;
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        if interactive {
            crate::commands::auth::login::perform_login().await?;
        } else {
            anyhow::bail!("Not logged in. Run `edgee auth login` first.");
        }
    }
    if interactive {
        crate::commands::auth::login::ensure_org_selected().await?;
    } else if creds.org_id.as_deref().unwrap_or("").is_empty() {
        anyhow::bail!("No organization selected. Run `edgee auth login` first.");
    }
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key(&provider)
        .await?
        .created;
    if reprovisioned && interactive {
        crate::commands::auth::login::ensure_onboarded(&provider).await?;
    }
    // VS Code can host Claude Code alongside Copilot chat. Provision the claude key
    // too so Claude's `/v1/messages` traffic reroutes through the claude pipeline.
    if is_copilot_vscode(&agent) {
        let reprov = crate::commands::auth::login::ensure_valid_provider_key("claude")
            .await?
            .created;
        if reprov && interactive {
            crate::commands::auth::login::ensure_onboarded("claude").await?;
        }
    }
    creds = crate::config::read()?;

    let api_key = creds
        .provider_api_key(&provider)
        .ok_or_else(|| anyhow::anyhow!("no Edgee API key for '{provider}'; run `edgee auth login`"))?
        .to_string();
    // Only wired for the VS Code relay; None elsewhere so `/v1/messages` keeps using
    // the relay's own key.
    let claude_api_key = if is_copilot_vscode(&agent) {
        creds.provider_api_key("claude").map(str::to_string)
    } else {
        None
    };
    let session_id = uuid::Uuid::new_v4().to_string();
    let repo = crate::git::detect_origin();

    let gateway_url = crate::commands::launch::resolve_gateway_base_url(&creds).await;
    // GUI editors have no Edgee provider pipeline; the gateway forwards their
    // rerouted calls to the editor's own backend, so record the original upstream.
    let passthrough_to_upstream = is_gui_editor(&agent);
    let debug_log_headers = crate::commands::launch::util::resolve_debug_log_keypair()?.map(|k| k.header_values());
    let gateway = build_gateway_target(
        &gateway_url,
        api_key,
        session_id.clone(),
        repo,
        passthrough_to_upstream,
        claude_api_key,
        debug_log_headers,
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

    // Run proxy-only when no agent is named, or when `--no-launch` is set (e.g.
    // driving an external client like Claude Desktop against a named provider's
    // pipeline). Otherwise launch the agent. Launch uses the canonical `agent`
    // (e.g. `vscode` → `copilot-vscode`), not the raw user input, so GUI-editor
    // detection and binary resolution work.
    if opts.agent.is_none() || opts.no_launch {
        print_external_help(&addr, &cert_path);
        proxy.start().await.context("relay proxy error")?;
    } else if is_gui_editor(&agent) {
        print_gui_editor_hint(&agent);
        let task = tokio::spawn(async move {
            let _ = proxy.start().await;
        });
        // A GUI editor's `--wait` CLI returns when the *window* it opened closes,
        // not when the editor quits. Any other window stays open still carrying this
        // relay's proxy env, so tearing the proxy down here would leave its traffic
        // pointed at a dead port (no direct fallback). Keep serving until Ctrl-C
        // instead — the same lifetime as the proxy-only path above.
        //
        // Ctrl-C is raced against the editor rather than awaited afterwards: the
        // listener has to be registered before the editor can exit, or a signal
        // already delivered is missed and we'd wait for a second press. GUI editors
        // leave the terminal free, so their Ctrl-C really does reach us as SIGINT
        // (TUI agents keep the terminal in raw mode and swallow it themselves).
        let mut interrupt = std::pin::pin!(shutdown_signal());
        let mut editor = std::pin::pin!(run_agent(&agent, port, &cert_path, &session_id));
        let exited = tokio::select! {
            res = &mut editor => Some(res?),
            _ = &mut interrupt => None,
        };
        match exited {
            // Window closed cleanly — hold the proxy for the windows still open.
            Some(status) if status.success() => {
                print_relay_still_serving(&addr, &agent);
                interrupt.await;
                task.abort();
            }
            // The editor CLI failed (most often: not on PATH). Surface it rather
            // than sitting on a proxy nothing is pointed at.
            Some(status) => {
                task.abort();
                if let Some(code) = status.code() {
                    std::process::exit(code);
                }
            }
            // Ctrl-C during the session — stop everything.
            None => task.abort(),
        }
    } else if is_claude_desktop(&agent) {
        // Claude Desktop is a GUI app we spawn and must tear down ourselves (it
        // can't recover once the proxy dies), so — unlike the TUI agents below —
        // we race its exit against Ctrl-C and kill it explicitly.
        print_claude_desktop_hint();
        // Claude Desktop (Chromium) verifies API certs against the macOS system
        // trust store — it ignores NODE_EXTRA_CA_CERTS and the --ignore-certificate-*
        // switches. Trust our CA there once; it's name-constrained to anthropic.com,
        // so a persistent trust root can MITM nothing else. Idempotent, so only the
        // first launch prompts (`--untrust` removes it).
        ensure_ca_trusted(&cert_path)?;
        let mut agent_child = spawn_agent(&agent, port, &cert_path, &session_id)?;
        let task = tokio::spawn(async move {
            let _ = proxy.start().await;
        });
        let started = std::time::Instant::now();
        // Box the wait future so the ctrl_c arm can drop it and reclaim `&mut
        // agent_child` to kill the process.
        let mut wait = Box::pin(agent_child.wait());
        let status = tokio::select! {
            r = &mut wait => r.context("waiting for the agent process")?,
            _ = tokio::signal::ctrl_c() => {
                drop(wait);
                // On macOS the terminal delivers SIGINT to the whole foreground
                // process group, so the child may already be exiting on its own;
                // give it a brief grace period, then force-kill. Without this the
                // spawned app would be left pointing at a now-dead proxy whenever it
                // gets no group signal — a Windows GUI process (no console signal) or
                // `kill -INT <edgee-pid>` — breaking every request until it's quit.
                if tokio::time::timeout(std::time::Duration::from_secs(2), agent_child.wait())
                    .await
                    .is_err()
                {
                    let _ = agent_child.start_kill();
                    let _ = agent_child.wait().await;
                }
                task.abort();
                std::process::exit(130);
            }
        };
        drop(wait);
        task.abort();
        // Claude Desktop holds a single-instance lock: if an instance was already
        // running, the binary we just spawned hands off to it and exits ~instantly
        // with success — leaving that existing instance running WITHOUT the proxy.
        // Detect the near-instant success exit and tell the user, instead of exiting
        // 0 as if the relay were live.
        if is_claude_desktop(&agent)
            && status.success()
            && started.elapsed() < std::time::Duration::from_millis(1500)
        {
            eprintln!(
                "{}",
                style(
                    "Claude Desktop was already running, so this launch handed off to the \
                     existing instance — which is NOT behind the relay. Quit Claude \
                     completely (Cmd-Q / right-click the tray icon → Quit), then re-run \
                     `edgee launch claude-desktop`."
                )
                .yellow()
            );
            std::process::exit(1);
        }
        if let Some(code) = status.code() {
            std::process::exit(code);
        }
    } else {
        // TUI agents (Claude Code, Codex) run in this terminal and handle their own
        // Ctrl-C in raw mode, so we just wait for them to exit — no signal race and
        // no child kill, which would fight the agent's own SIGINT handling.
        let task = tokio::spawn(async move {
            let _ = proxy.start().await;
        });
        let status = run_agent(&agent, port, &cert_path, &session_id).await?;
        task.abort();
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
        untrust: false,
        non_interactive: false,
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

/// Parse the resolved gateway URL into a reroute target.
fn build_gateway_target(
    url: &str,
    api_key: String,
    session_id: String,
    repo: Option<String>,
    passthrough_to_upstream: bool,
    claude_api_key: Option<String>,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
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
        debug_log_headers,
    })
}

/// Common Name of the dedicated, name-constrained CA used only for the
/// `claude-desktop` relay. Kept distinct from the shared `Edgee CA` so
/// [`ensure_ca_trusted`] can add/remove exactly this cert in the keychain, and so
/// it never gets trusted for anything but Anthropic.
const CLAUDE_DESKTOP_CA_CN: &str = "Edgee Claude Desktop CA";

/// The macOS System keychain — the admin-domain trust store Claude Desktop's
/// Chromium net stack consults.
#[cfg(target_os = "macos")]
const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

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
        // Re-tighten perms on every load: an older CLI may have written the key
        // world-readable (default umask), so heal it even when we don't regenerate.
        restrict_private(&key_path);
        let cert = std::fs::read_to_string(&cert_path)
            .with_context(|| format!("reading CA cert {}", cert_path.display()))?;
        let key = std::fs::read_to_string(&key_path)
            .with_context(|| format!("reading CA key {}", key_path.display()))?;
        return Ok((cert, key, cert_path));
    }

    std::fs::create_dir_all(&dir).with_context(|| format!("creating CA dir {}", dir.display()))?;
    // The dir holds private keys; keep it owner-only.
    restrict_dir(&dir);
    let (cert_pem, key_pem) = generate_ca(common_name, permitted_dns)?;
    std::fs::write(&cert_path, &cert_pem)
        .with_context(|| format!("writing CA cert {}", cert_path.display()))?;
    // Create the key `0600` from the start (never briefly world-readable).
    write_private(&key_path, &key_pem)
        .with_context(|| format!("writing CA key {}", key_path.display()))?;
    Ok((cert_pem, key_pem, cert_path))
}

/// Write `contents` to `path` as an owner-only (`0600`) file, created with those
/// permissions from the outset so the key is never momentarily world-readable.
/// On non-Unix, falls back to a plain write (Windows relies on profile ACLs).
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

/// Best-effort tighten an existing private-key file to `0600` (no-op off Unix).
fn restrict_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Best-effort tighten the CA directory to `0700` (no-op off Unix).
fn restrict_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Generate a self-signed CA suitable for signing leaf certs at runtime. When
/// `permitted_dns` is non-empty the CA carries an X.509 name-constraints extension
/// limiting the DNS names it may issue certificates for (RFC 5280); a constraint of
/// `anthropic.com` also permits `api.anthropic.com` and other subdomains.
fn generate_ca(common_name: &str, permitted_dns: &[&str]) -> Result<(String, String)> {
    use rcgen::{
        BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType, GeneralSubtree,
        IsCa, KeyPair, KeyUsagePurpose, NameConstraints,
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
            // Permitting only a dNSName subtree leaves every *other* name form
            // unconstrained (RFC 5280 §4.2.1.10: a form with no subtree present is
            // unrestricted), so a leaf carrying an iPAddress SAN would chain validly
            // and bypass the DNS limit. Exclude the entire IPv4/IPv6 space (an
            // all-zero mask matches every address) so this CA can never vouch for an
            // IP literal — the legit relay leaves only ever use DNS SANs.
            excluded_subtrees: vec![
                GeneralSubtree::IpAddress(CidrSubnet::V4([0, 0, 0, 0], [0, 0, 0, 0])),
                GeneralSubtree::IpAddress(CidrSubnet::V6([0; 16], [0; 16])),
            ],
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

/// Ensure the name-constrained claude-desktop CA is trusted in the macOS **System**
/// keychain, installing it **once** if needed. Claude Desktop (Chromium) verifies
/// against the OS trust store only, and this CA is constrained to `anthropic.com`, so
/// a persistent trust root here can MITM nothing but Anthropic traffic — cheap enough
/// to install once and leave, rather than re-prompt for `sudo` on every launch.
///
/// Idempotent: if the exact CA (matched by SHA-1, so a regenerated CA is caught) is
/// already trusted, it returns without prompting. Otherwise it purges any stale copy
/// and adds the current CA — one `sudo` prompt, on the first launch only. Remove it
/// anytime with `edgee relay claude-desktop --untrust`. No-op off macOS.
fn ensure_ca_trusted(ca_path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if ca_is_trusted(ca_path) {
            return Ok(());
        }
        // Not trusted (first launch, or the CA was regenerated) → (re)install it.
        remove_trusted_ca();
        eprintln!(
            "{}",
            style(
                "Trusting the Edgee Claude Desktop CA in the system keychain \
                 (one-time, admin required)…"
            )
            .dim()
        );
        let status = std::process::Command::new("sudo")
            .args([
                "security",
                "add-trusted-cert",
                "-d",
                "-r",
                "trustRoot",
                // Scope trust to the SSL (TLS server) policy only. Without `-p`,
                // add-trusted-cert blesses the root for *every* policy (S/MIME,
                // code-signing, …), which our DNS-only name constraint doesn't
                // limit — so a stolen key could mint a trusted S/MIME or signing
                // cert. TLS is all Claude Desktop needs.
                "-p",
                "ssl",
                "-k",
                SYSTEM_KEYCHAIN,
            ])
            .arg(ca_path)
            .status()
            .context("running `sudo security add-trusted-cert`")?;
        if !status.success() {
            anyhow::bail!(
                "failed to trust the Edgee Claude Desktop CA in the system keychain \
                 (Claude Desktop needs it to accept the relay)."
            );
        }
        // Confirm the cert now trusted is exactly the one we intended to install.
        // Closes the window between the fingerprint check above and `sudo` re-reading
        // the file: if anything swapped it underneath us, fail loudly rather than
        // leave an unexpected root blessed.
        if !ca_is_trusted(ca_path) {
            anyhow::bail!(
                "the Edgee Claude Desktop CA did not verify as trusted after install \
                 (the cert file may have changed underneath us). Re-run, or clear any \
                 stray cert with `edgee relay claude-desktop --untrust`."
            );
        }
        eprintln!(
            "{}",
            style(
                "Done — future launches won't prompt. Remove anytime with \
                 `edgee relay claude-desktop --untrust`."
            )
            .dim()
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ca_path;
    }
    Ok(())
}

/// True when a cert matching `ca_path` (by SHA-1) is already trusted in the System
/// keychain, so subsequent launches can skip the re-install. Matching by fingerprint
/// (not just Common Name) means a regenerated CA correctly reads as "not trusted"
/// and gets refreshed.
#[cfg(target_os = "macos")]
fn ca_is_trusted(ca_path: &Path) -> bool {
    let Some(disk_sha1) = cert_sha1(ca_path) else {
        return false;
    };
    // `-a` lists *every* cert with this CN, not just the first match, so an
    // accumulated stale duplicate can't hide the current one (or vice-versa).
    let Ok(out) = std::process::Command::new("security")
        .args(["find-certificate", "-a", "-c", CLAUDE_DESKTOP_CA_CN, "-Z", SYSTEM_KEYCHAIN])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().strip_prefix("SHA-1 hash: "))
        .any(|h| h.trim().eq_ignore_ascii_case(&disk_sha1))
}

/// True when any cert with the claude-desktop CN is present in the System keychain.
/// Used to tell "nothing to remove" from "removal failed" in the untrust path.
#[cfg(target_os = "macos")]
fn ca_present() -> bool {
    std::process::Command::new("security")
        .args(["find-certificate", "-a", "-c", CLAUDE_DESKTOP_CA_CN, SYSTEM_KEYCHAIN])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// SHA-1 fingerprint of the DER certificate in the PEM at `path`, as uppercase hex
/// with no separators — the exact form `security … -Z` prints, so trust-matching
/// needs no external tools. `None` if the file can't be read or holds no cert.
/// (SHA-1 here only mirrors the keychain's own fingerprint index; it is not a
/// collision-adversary boundary.)
#[cfg(target_os = "macos")]
fn cert_sha1(path: &Path) -> Option<String> {
    use sha1::{Digest, Sha1};

    let pem = std::fs::read_to_string(path).ok()?;
    let der = pem_first_block_der(&pem)?;
    let digest = Sha1::digest(&der);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        hex.push_str(&format!("{b:02X}"));
    }
    Some(hex)
}

/// Decode the first PEM block's base64 body to DER. We only ever hash our own
/// freshly generated single-cert CA file, so a minimal BEGIN/END scan suffices.
#[cfg(target_os = "macos")]
fn pem_first_block_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine;

    let begin = pem.find("-----BEGIN")?;
    let body_start = pem[begin..].find('\n')? + begin + 1;
    let end = pem[body_start..].find("-----END")? + body_start;
    let body: String = pem[body_start..end].split_whitespace().collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// Remove the claude-desktop CA from the System keychain (best-effort; a missing
/// cert is fine). Targets the dedicated, name-constrained CA by CN. `delete-certificate`
/// removes only one match per call, so loop until none remain — a stale duplicate
/// must never survive as a trusted root. The bounded count guards against an
/// unexpected non-deleting exit spinning forever.
#[cfg(target_os = "macos")]
fn remove_trusted_ca() {
    for _ in 0..16 {
        if !ca_present() {
            return;
        }
        let deleted = std::process::Command::new("sudo")
            .args([
                "security",
                "delete-certificate",
                "-c",
                CLAUDE_DESKTOP_CA_CN,
                SYSTEM_KEYCHAIN,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !deleted {
            // sudo denied or errored — stop; the caller re-checks presence and
            // reports accordingly rather than looping on a failing command.
            return;
        }
    }
}

/// `edgee relay claude-desktop --untrust`: remove the trusted claude-desktop CA.
/// Reports whether anything was actually removed and fails (non-zero exit) if a
/// cert is still present afterward — for a command whose whole job is revoking a
/// system trust root, a silent success on a denied `sudo` would be dangerous.
fn untrust_ca() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if !ca_present() {
            eprintln!(
                "{}",
                style("No Edgee Claude Desktop CA is trusted — nothing to remove.").dim()
            );
            return Ok(());
        }
        eprintln!(
            "{}",
            style("Removing the Edgee Claude Desktop CA from the system keychain…").dim()
        );
        remove_trusted_ca();
        if ca_present() {
            anyhow::bail!(
                "failed to remove the Edgee Claude Desktop CA from the system keychain \
                 (admin authorization is required)."
            );
        }
        eprintln!("{}", style("Removed.").dim());
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("Nothing to remove (macOS only).");
    }
    Ok(())
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

/// Spawn the named agent wired through the proxy and return the live child handle
/// (the caller awaits it, and kills it on Ctrl-C). The proxy injects Edgee auth on
/// reroute, so no base-URL / custom-header env is needed here.
fn spawn_agent(
    agent: &str,
    port: u16,
    ca_path: &Path,
    session_id: &str,
) -> Result<tokio::process::Child> {
    let proxy_url = format!("http://127.0.0.1:{port}");

    // Claude Desktop is a GUI Electron app with no CLI shim, so we spawn the app
    // bundle's own binary directly (not `open`/a wrapper) — that keeps this process
    // attached until the app quits (holding the proxy open) and lets the proxy env
    // propagate into it. Like Cursor, its Electron net stack won't honor
    // HTTPS_PROXY, so route it explicitly with Chromium's --proxy-server. Its API
    // calls verify against the macOS system trust store (Chromium net ignores
    // NODE_EXTRA_CA_CERTS *and* the --ignore-certificate-errors* switches when
    // passed via argv), so the relay CA must be trusted in the keychain — handled
    // in the System keychain by `ensure_ca_trusted` in `run`.
    let mut cmd = if is_claude_desktop(agent) {
        let bin = claude_desktop_binary()?;
        let mut c = tokio::process::Command::new(bin);
        c.arg(format!("--proxy-server={proxy_url}"));
        // Claude Desktop is a Chromium/Electron GUI app; launched from a terminal it
        // spews a firehose of browser-process logs to stdout/stderr. None of it is
        // actionable for the relay session, so silence the child's stdio (a GUI app
        // needs no stdin either). This does NOT touch the relay's own traffic logging,
        // which happens in the proxy, not the child. The `--wait` editors don't need
        // this — their CLI shims stay quiet — and TUI agents must keep inherited stdio.
        c.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
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

    cmd.spawn().with_context(|| {
        // A GUI editor most often fails here because its CLI isn't on PATH.
        match macos_cli_path_hint(agent) {
            Some(hint) => format!("failed to launch '{agent}'. {}", hint.join(" ")),
            None => format!("failed to launch '{agent}'"),
        }
    })
}

/// Spawn the agent and wait for it to exit. Used by the GUI-editor and TUI paths,
/// which run the process to completion rather than racing Ctrl-C to kill it (the
/// Claude Desktop path holds the [`Child`](tokio::process::Child) from
/// [`spawn_agent`] directly so it can force-kill on interrupt).
async fn run_agent(
    agent: &str,
    port: u16,
    ca_path: &Path,
    session_id: &str,
) -> Result<std::process::ExitStatus> {
    Ok(spawn_agent(agent, port, ca_path, session_id)?.wait().await?)
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

/// Hint printed before launching Claude Desktop behind the relay. Unlike the
/// passthrough editors, Claude Desktop is a GUI app we spawn ourselves, so we just
/// tell the user to quit any pre-existing instance first — the proxy env only
/// reaches a freshly spawned process.
fn print_claude_desktop_hint() {
    println!(
        "{}",
        style("Launching Claude Desktop (the Claude app) behind the relay.").bold()
    );
    println!(
        "  {}",
        style("Quit any running Claude Desktop first — the proxy env only applies to a").dim()
    );
    println!(
        "  {}",
        style("freshly spawned instance. Its traffic then reroutes through the gateway.").dim()
    );
    println!();
}

fn print_gui_editor_hint(agent: &str) {
    let app = editor_app_name(agent);
    let (cli, launch, feature) = if is_cursor(agent) {
        ("cursor", "cursor --wait", "Cursor AI")
    } else {
        ("code", "code --wait", "Copilot Chat")
    };
    println!(
        "{}",
        style(format!("Launching {app} ({launch}) behind the relay.")).bold()
    );
    println!(
        "  {}",
        style(format!(
            "Quit any running {app} first: `{cli}` hands the request to the instance"
        ))
        .dim()
    );
    println!(
        "  {}",
        style("already running, which was started without the relay's proxy env — its").dim()
    );
    println!(
        "  {}",
        style(format!(
            "{feature} traffic would bypass the gateway entirely. A freshly spawned"
        ))
        .dim()
    );
    println!("  {}", style("instance reroutes through it.").dim());
    for line in macos_cli_path_hint(agent).unwrap_or_default() {
        println!("  {}", style(line).dim());
    }
    println!();
}

/// Printed when a GUI editor's `--wait` CLI returns (the window it opened closed)
/// while the relay stays up for whatever windows are still open. Says why the
/// proxy is still listening, and what stopping it costs.
fn print_relay_still_serving(addr: &SocketAddr, agent: &str) {
    let app = editor_app_name(agent);
    println!();
    println!(
        "{}",
        style(format!(
            "{app} window closed — relay still serving on http://{addr}"
        ))
        .bold()
    );
    println!(
        "  {}",
        style(format!(
            "Any other {app} window is still routed through it. Ctrl-C to stop the"
        ))
        .dim()
    );
    println!(
        "  {}",
        style(format!(
            "relay — {app} windows left open then lose gateway routing."
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
            build_gateway_target("https://edgee.io", "k".into(), "s".into(), None, false, None, None)
                .unwrap();
        assert_eq!(gw.scheme, Scheme::HTTPS);
        assert_eq!(gw.authority.as_str(), "edgee.io");
        assert_eq!(gw.base_path, "");
    }

    #[test]
    fn parses_local_override() {
        let gw =
            build_gateway_target("http://127.0.0.1:9999", "k".into(), "s".into(), None, false, None, None)
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
    fn claude_desktop_is_not_a_passthrough_editor() {
        assert!(is_claude_desktop("claude-desktop"));
        // It's a GUI app we launch, but NOT a passthrough editor: it routes
        // through the real Claude provider pipeline, so passthrough stays off.
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
    #[cfg(target_os = "macos")]
    fn cert_sha1_parses_pem_to_forty_hex_uppercase() {
        // The in-process fingerprint must match the keychain's `-Z` form: 40
        // uppercase hex chars, no separators. Guards the PEM→DER→SHA-1 path that
        // replaced the external `openssl` call.
        let (pem, _) = generate_ca(CLAUDE_DESKTOP_CA_CN, &["anthropic.com"]).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ca.pem");
        std::fs::write(&path, &pem).unwrap();
        let sha1 = cert_sha1(&path).expect("fingerprint");
        assert_eq!(sha1.len(), 40, "SHA-1 hex is 40 chars, got {sha1:?}");
        assert!(
            sha1.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()),
            "expected uppercase hex, got {sha1:?}"
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
    #[cfg(target_os = "macos")]
    fn macos_path_hint_targets_the_right_editor_cli() {
        let cursor = macos_cli_path_hint("cursor").unwrap().join(" ");
        assert!(cursor.contains("Install 'cursor' command"));
        let vscode = macos_cli_path_hint("copilot-vscode").unwrap().join(" ");
        assert!(vscode.contains("Shell Command: Install 'code' command in PATH"));
        // TUI agents resolve their own binary — no editor Command Palette to point at.
        assert!(macos_cli_path_hint("claude").is_none());
        assert!(macos_cli_path_hint("codex").is_none());
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn path_hint_is_macos_only() {
        assert!(macos_cli_path_hint("cursor").is_none());
        assert!(macos_cli_path_hint("copilot-vscode").is_none());
    }

    #[test]
    fn editor_app_names_match_their_target() {
        assert_eq!(editor_app_name("cursor"), "Cursor");
        for a in ["copilot-vscode", "vscode-copilot", "code", "vscode"] {
            let canon = canonicalize_target(a).unwrap();
            assert_eq!(editor_app_name(canon), "VS Code", "{a}");
        }
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
            build_gateway_target("/no/host", "k".into(), "s".into(), None, false, None, None).is_err()
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

