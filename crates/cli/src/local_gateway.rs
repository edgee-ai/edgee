//! Core local gateway server logic.
//!
//! Provides [`start()`] which binds a TCP listener and spawns an Axum server
//! in a background tokio task. Used by both the `edgee local-gateway`
//! subcommand and the `--local-gateway` flag on `edgee launch {claude,codex}`.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use axum_core::response::IntoResponse;
use tokio::sync::oneshot;
use tower::ServiceBuilder;

use edgee_compression_layer::{CompressionConfig, CompressionLayer};
use edgee_gateway_core::{
    passthrough::{anthropic::AnthropicPassthroughService, openai::OpenAIPassthroughService},
    HttpClient, ReqwestHttpClient,
};
use edgee_gateway_http::{Error, PassthroughLayer};

/// Bound for the TCP/TLS handshake against the upstream provider.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-read inactivity timeout against the upstream provider. Applied per
/// chunk, so it bounds how long the provider can be silent without aborting
/// long-lived streaming responses (SSE keepalives reset the timer).
const UPSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Handle to a running local gateway instance.
///
/// The server lives in its own tokio task. Call [`LocalGatewayHandle::shutdown`]
/// to send the stop signal; this is fire-and-forget and does not wait for drain.
pub struct LocalGatewayHandle {
    /// The socket address the server is actually bound to. Useful when port 0
    /// was requested and the OS assigned an ephemeral port.
    pub addr: SocketAddr,
    pub(crate) shutdown_tx: oneshot::Sender<()>,
    /// Join handle for the server task. Resolves when the server exits, carrying
    /// any error that caused an unexpected shutdown.
    pub(crate) task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl LocalGatewayHandle {
    /// Send the shutdown signal. Fire-and-forget; does not wait for the server
    /// task to drain. Safe to call after the agent process has exited.
    pub fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

/// Start a local gateway server in a background tokio task.
///
/// Binds a [`tokio::net::TcpListener`] before returning, so the port is live
/// and ready to accept connections before the caller proceeds. Pass `port = 0`
/// to let the OS assign an available ephemeral port.
pub async fn start(addr: SocketAddr) -> Result<LocalGatewayHandle> {
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
        .layer(CompressionLayer::new(CompressionConfig::claude().build()))
        .service(
            AnthropicPassthroughService::builder()
                .client(http_client.clone())
                .build(),
        );

    let openai = ServiceBuilder::new()
        .layer(axum::error_handling::HandleErrorLayer::new(
            |e: Error| async move { e.into_response() },
        ))
        .layer(PassthroughLayer::new())
        .layer(CompressionLayer::new(CompressionConfig::codex().build()))
        .service(
            OpenAIPassthroughService::builder()
                .client(http_client.clone())
                .build(),
        );

    let app = Router::new()
        .route_service("/v1/messages", anthropic)
        .route_service("/v1/responses", openai);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind local gateway on {addr}"))?;
    let bound_addr = listener.local_addr().context("failed to get local_addr")?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
            .context("local gateway server error")
    });

    Ok(LocalGatewayHandle {
        addr: bound_addr,
        shutdown_tx,
        task,
    })
}

/// Initialise tracing with a file writer. Creates parent directories as needed.
/// Uses EDGEE_GATEWAY_LOG env-var for the filter; falls back to
/// `warn,edgee_gateway_http=info,edgee_cli=info`.
pub fn init_file_tracing(log_path: &Path) -> anyhow::Result<()> {
    use tracing_subscriber::EnvFilter;

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    let filter =
        EnvFilter::try_from_env("EDGEE_GATEWAY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .with_writer(file)
        .try_init();

    Ok(())
}
