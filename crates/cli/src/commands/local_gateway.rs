//! `edgee local-gateway` subcommand.
//!
//! Thin CLI wrapper around [`crate::local_gateway::start`]. Runs the gateway
//! until Ctrl+C, then signals a graceful shutdown.
//!
//! Routes:
//!   POST /v1/messages  → Anthropic Messages API (passthrough + compression)
//!   POST /v1/responses → OpenAI Responses API   (passthrough + compression)
//!
//! Local dev only. No auth, no TLS, no rate limiting.

use std::net::IpAddr;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Port to bind
    #[arg(long, default_value_t = 8787)]
    pub port: u16,

    /// Address to bind
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: IpAddr,
}

pub async fn run(opts: Options) -> Result<()> {
    init_tracing();

    if !opts.bind.is_loopback() {
        let bind = opts.bind;
        eprintln!(
            "WARNING: binding to non-loopback address {bind}: this gateway has no \
             auth, no TLS, and no rate limiting. Anyone on the network can use \
             it as an unauthenticated proxy and may be able to intercept the \
             API keys it forwards.",
        );
    }

    let handle = crate::local_gateway::start((opts.bind, opts.port).into()).await?;
    let addr = handle.addr;
    eprintln!("edgee local-gateway listening on http://{addr}");
    eprintln!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down…");
    handle.shutdown();

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("EDGEE_GATEWAY_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,edgee_gateway_http=info,edgee_cli=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}
