//! `edgee relay` — a local MITM proxy that logs LLM API traffic and reroutes
//! inference requests through the Edgee gateway.
//!
//! Terminates TLS with a locally-generated CA so HTTPS headers and bodies are
//! visible. Requests to inference paths (`/v1/messages`, `/v1/responses`,
//! `/v1/chat/completions`) on known LLM hosts are rewritten to the Edgee gateway
//! (with `x-edgee-*` auth injected); everything else is forwarded to its original
//! upstream. All matching traffic is logged.

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

/// Provider hosts logged by default. Inference paths on these hosts are also the
/// only ones rerouted to the gateway.
const DEFAULT_DOMAINS: &[&str] = &[
    "api.anthropic.com",
    "api.openai.com",
    "chatgpt.com",
];

const PROVIDERS: &[&str] = &["claude", "codex"];

setup_command! {
    /// Agent to spawn through the relay (claude|codex). Omit to run proxy-only
    /// (e.g. for external clients like Claude Desktop). Selects which Edgee key
    /// is injected (claude by default).
    pub agent: Option<String>,
    /// Port the proxy listens on. Defaults per agent (claude 41100, codex 41200)
    /// so `relay claude` and `relay codex` can run side by side.
    #[arg(long)]
    pub port: Option<u16>,
    /// Host to log; repeatable. Defaults to the LLM provider list.
    #[arg(long = "domain")]
    pub domains: Vec<String>,
    /// Write relayed-traffic logs to this file (appended). If unset, logging is off.
    #[arg(long)]
    pub log_output: Option<PathBuf>,
}

pub async fn run(opts: Options) -> Result<()> {
    // The injected Edgee key follows the relay's agent: claude when claude is the
    // agent (or proxy-only), codex when codex is. Each spawned agent routes only
    // through its own relay, so concurrent `relay claude` / `relay codex` (on
    // different ports) each inject the right key without interference.
    let provider = opts.agent.clone().unwrap_or_else(|| "claude".to_string());
    if !PROVIDERS.contains(&provider.as_str()) {
        anyhow::bail!("unknown agent '{provider}' (expected claude|codex)");
    }

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
    creds = crate::config::read()?;

    let api_key = provider_api_key(&creds, &provider).ok_or_else(|| {
        anyhow::anyhow!("no Edgee API key for '{provider}'; run `edgee auth login`")
    })?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let repo = crate::git::detect_origin();

    let gateway_url = crate::commands::launch::resolve_gateway_base_url(&creds).await;
    let gateway = build_gateway_target(&gateway_url, api_key, session_id.clone(), repo)?;

    let (cert_pem, key_pem, cert_path) = ensure_ca()?;
    let domains: Vec<String> = if opts.domains.is_empty() {
        DEFAULT_DOMAINS.iter().map(|s| s.to_string()).collect()
    } else {
        opts.domains.clone()
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

    let handler = RelayHandler::new(
        Arc::new(domains.clone()),
        sink,
        Arc::new(gateway.clone()),
        log_enabled,
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
        &domains,
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
        port: None,
        domains: Vec::new(),
        log_output: None,
    })
    .await
}

/// Default listen port per agent, picked from an uncommon range so two relays
/// (`relay claude` + `relay codex`) don't collide out of the box.
fn default_port(provider: &str) -> u16 {
    match provider {
        "codex" => 41200,
        _ => 41100, // claude / proxy-only
    }
}

/// The Edgee key for `provider` from the active profile, if present.
fn provider_api_key(creds: &crate::config::Credentials, provider: &str) -> Option<String> {
    let p = match provider {
        "claude" => creds.claude.as_ref(),
        "codex" => creds.codex.as_ref(),
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

/// Spawn the named agent wired through the proxy. The proxy injects Edgee auth on
/// reroute, so no base-URL / custom-header env is needed here.
async fn run_agent(
    agent: &str,
    port: u16,
    ca_path: &Path,
    session_id: &str,
) -> Result<std::process::ExitStatus> {
    let bin = crate::commands::launch::util::resolve_binary(agent);
    let proxy_url = format!("http://127.0.0.1:{port}");

    let mut cmd = tokio::process::Command::new(bin);
    cmd.env("HTTPS_PROXY", &proxy_url);
    cmd.env("HTTP_PROXY", &proxy_url);
    cmd.env("https_proxy", &proxy_url);
    cmd.env("http_proxy", &proxy_url);
    // Make each agent's TLS stack trust the relay CA without a system-store install:
    //  - Node agents (Claude Code) read NODE_EXTRA_CA_CERTS.
    //  - Codex (Rust) reads CODEX_CA_CERTIFICATE / SSL_CERT_FILE for its own client;
    //    it does NOT read NODE_EXTRA_CA_CERTS.
    cmd.env("NODE_EXTRA_CA_CERTS", ca_path);
    cmd.env("CODEX_CA_CERTIFICATE", ca_path);
    cmd.env("EDGEE_SESSION_ID", session_id);

    cmd.status()
        .await
        .with_context(|| format!("failed to launch '{agent}'"))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn print_banner(
    addr: &SocketAddr,
    cert_path: &Path,
    domains: &[String],
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
    println!("  domains:  {}", domains.join(", "));
    match log_output {
        Some(p) => println!("  logs:     {}", p.display()),
        None => println!("  logs:     disabled"),
    }
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
        let gw = build_gateway_target("https://edgee.io", "k".into(), "s".into(), None).unwrap();
        assert_eq!(gw.scheme, Scheme::HTTPS);
        assert_eq!(gw.authority.as_str(), "edgee.io");
        assert_eq!(gw.base_path, "");
    }

    #[test]
    fn parses_local_override() {
        let gw =
            build_gateway_target("http://127.0.0.1:9999", "k".into(), "s".into(), None).unwrap();
        assert_eq!(gw.scheme.as_str(), "http");
        assert_eq!(gw.authority.as_str(), "127.0.0.1:9999");
        assert_eq!(gw.base_path, "");
    }

    #[test]
    fn rejects_url_without_host() {
        // A path-only URI has no authority → reroute target can't be built.
        assert!(build_gateway_target("/no/host", "k".into(), "s".into(), None).is_err());
    }
}

