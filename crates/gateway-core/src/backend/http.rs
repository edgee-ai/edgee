use axum_core::body::Body;
use http::{Request, Response};

use crate::error::{Error, Result};

/// Abstract HTTP transport.
///
/// Implementations exist for:
/// - [`ReqwestHttpClient`] (tokio feature, local/AWS backends)
/// - Platform-specific clients (e.g. Fastly backend — no tokio/reqwest required)
///
/// Callers inject a concrete implementation at construction time via
/// [`crate::service::ProviderDispatchService::new`] or the passthrough services.
/// The core crate itself never depends on a specific runtime.
#[async_trait::async_trait]
pub trait HttpClient: Send + Sync {
    async fn send(&self, req: Request<Body>) -> Result<Response<Body>>;
}

/// A [`HttpClient`] backed by [`reqwest`].
///
/// Only available when the `tokio` feature is enabled. Use this in local
/// development and on platforms that support the tokio async runtime (e.g. AWS).
///
/// For Fastly Compute@Edge (`wasm32-wasip1`), provide your own [`HttpClient`]
/// implementation using the Fastly SDK instead.
#[cfg(feature = "tokio")]
pub struct ReqwestHttpClient(reqwest::Client);

#[cfg(feature = "tokio")]
impl ReqwestHttpClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self(client)
    }
}

#[cfg(feature = "tokio")]
#[async_trait::async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn send(&self, req: Request<Body>) -> Result<Response<Body>> {
        let req: reqwest::Request = req
            .map(|body| reqwest::Body::wrap_stream(body.into_data_stream()))
            .try_into()
            .map_err(|e| Error::HttpClient(format!("Failed to convert request: {e}")))?;

        let resp = self
            .0
            .execute(req)
            .await
            .map_err(|e| Error::HttpClient(format!("HTTP request failed: {e}")))?;
        let resp = Response::from(resp);

        Ok(resp.map(Body::new))
    }
}
