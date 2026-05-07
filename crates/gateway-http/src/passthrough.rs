//! Gateway passthrough service for HTTP requests.
//!
//! This module provides a passthrough service that forwards HTTP requests to their intended destinations without modification.
//! It is designed to be used to redirect LLM requests to the appropriate backend services while maintaining the original request structure.

use std::task::{Context, Poll};

use axum_core::body::Body;
use futures::future::BoxFuture;
use http::{Request, Response};
use http_body_util::{BodyExt as _, Limited};
use tower::Service;
use tracing::Instrument as _;

use edgee_gateway_core::PassthroughRequest;

use crate::error::Error;

/// Maximum accepted request body size, in bytes. Requests larger than this are
/// rejected with HTTP 413 before the body is buffered into memory.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Tower service that converts an incoming [`Request<Body>`] into a [`PassthroughRequest`]
/// and forwards it to an inner service.
///
/// Responsibilities at this HTTP boundary:
/// - Read and parse the request body as JSON.
/// - Strip hop-by-hop and gateway-internal headers (see [`SKIP_HEADERS`]).
/// - Delegate to the inner service with the resulting [`PassthroughRequest`].
#[derive(Clone)]
pub struct PassthroughService<S> {
    inner: S,
}

impl<S> PassthroughService<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> Service<Request<Body>> for PassthroughService<S>
where
    S: Service<PassthroughRequest, Response = Response<Body>> + Clone + Send + 'static,
    S::Error: Into<Error>,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Error;
    type Future = BoxFuture<'static, Result<Self::Response, Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        use edgee_gateway_core::passthrough::SKIP_HEADERS;

        let mut inner = self.inner.clone();
        let span = tracing::info_span!(
            "gateway.http.passthrough",
            "gen_ai.request.model" = tracing::field::Empty,
            "gen_ai.request.stream" = tracing::field::Empty,
        );

        Box::pin(
            async move {
                let (parts, body) = req.into_parts();

                let bytes = Limited::new(body, MAX_BODY_BYTES)
                    .collect()
                    .await
                    .map_err(crate::error::body_read_error)?
                    .to_bytes();

                let json_body = serde_json::from_slice::<serde_json::Value>(&bytes)?;

                let current = tracing::Span::current();
                if let Some(model) = json_body.get("model").and_then(|v| v.as_str()) {
                    current.record("gen_ai.request.model", model);
                }
                if let Some(stream) = json_body.get("stream").and_then(|v| v.as_bool()) {
                    current.record("gen_ai.request.stream", stream);
                }

                let mut headers = http::HeaderMap::new();
                for (name, value) in &parts.headers {
                    if !SKIP_HEADERS.contains(&name.as_str()) {
                        headers.insert(name.clone(), value.clone());
                    }
                }

                tracing::debug!("forwarding passthrough request");

                let passthrough_req = PassthroughRequest::new(json_body, headers);

                inner.call(passthrough_req).await.map_err(Into::into)
            }
            .instrument(span),
        )
    }
}

/// Tower [`Layer`](tower::Layer) that wraps a service with [`PassthroughService`].
#[derive(Clone, Default)]
pub struct PassthroughLayer;

impl PassthroughLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> tower::Layer<S> for PassthroughLayer {
    type Service = PassthroughService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PassthroughService::new(inner)
    }
}
