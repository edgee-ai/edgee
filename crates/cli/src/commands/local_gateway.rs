//! `edgee local-gateway` subcommand.
//!
//! Runs a minimal HTTP server bound to a local port that routes incoming LLM
//! requests through the Edgee passthrough + compression pipeline before
//! forwarding to the upstream provider.
//!
//! Routes:
//!   POST /v1/messages  → Anthropic Messages API (passthrough + compression)
//!   POST /v1/responses → OpenAI Responses API   (passthrough + compression)
//!
//! Local dev only. No auth, no TLS, no rate limiting.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use axum::Router;
use axum_core::response::IntoResponse;
use edgee_compression_layer::{AgentType, CompressionConfig, PassthroughCompressionLayer};
use edgee_gateway_core::{
    AnthropicPassthroughConfig, HttpClient, OpenAIPassthroughConfig, ReqwestHttpClient,
    passthrough::{anthropic::AnthropicPassthroughService, openai::OpenAIPassthroughService},
};
use edgee_gateway_http::{Error, PassthroughLayer};
use tower::ServiceBuilder;
use tracing_subscriber::EnvFilter;

/// Bound for the TCP/TLS handshake against the upstream provider.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-read inactivity timeout against the upstream provider. Applied per
/// chunk, so it bounds how long the provider can be silent without aborting
/// long-lived streaming responses (SSE keepalives reset the timer).
const UPSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(30);

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

    let addr = SocketAddr::new(opts.bind, opts.port);

    if !opts.bind.is_loopback() {
        eprintln!(
            "WARNING: binding to non-loopback address {}: this gateway has no \
             auth, no TLS, and no rate limiting. Anyone on the network can use \
             it as an unauthenticated proxy and may be able to intercept the \
             API keys it forwards.",
            opts.bind
        );
    }

    let reqwest_client = reqwest::Client::builder()
        .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
        .read_timeout(UPSTREAM_READ_TIMEOUT)
        .build()
        .context("failed to build reqwest client")?;
    let http_client: Arc<dyn HttpClient> = Arc::new(ReqwestHttpClient::new(reqwest_client));

    let anthropic = ServiceBuilder::new()
        .layer(axum::error_handling::HandleErrorLayer::new(
            |e: Error| async move { e.into_response() },
        ))
        .layer(PassthroughLayer::new())
        .layer(PassthroughCompressionLayer::new(CompressionConfig::new(
            AgentType::Claude,
        )))
        .service(AnthropicPassthroughService::new(
            http_client.clone(),
            AnthropicPassthroughConfig::default(),
        ));

    let openai = ServiceBuilder::new()
        .layer(axum::error_handling::HandleErrorLayer::new(
            |e: Error| async move { e.into_response() },
        ))
        .layer(PassthroughLayer::new())
        .layer(PassthroughCompressionLayer::new(CompressionConfig::new(
            AgentType::Codex,
        )))
        .service(OpenAIPassthroughService::new(
            http_client.clone(),
            OpenAIPassthroughConfig::default(),
        ));

    let app = Router::new()
        .route_service("/v1/messages", anthropic)
        .route_service("/v1/responses", openai);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    eprintln!("edgee local-gateway listening on http://{addr}");
    eprintln!("Press Ctrl+C to stop.");

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("gateway server error")?;

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

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("\nShutting down…");
}
