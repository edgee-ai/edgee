use std::{
    sync::Arc,
    task::{Context, Poll},
};

use edgee_gateway_core::{CompletionRequest, PassthroughRequest};
use tower::Service;

use crate::{compress::compress_request, config::CompressionConfig};

/// Tower [`Service`] produced by [`CompressionLayer`](crate::CompressionLayer).
///
/// Intercepts each request, compresses tool-result content in-place, then
/// delegates to the wrapped inner service. Implements [`Service<CompletionRequest>`]
/// for the typed dispatch path and [`Service<PassthroughRequest>`] for the
/// provider-native JSON passthrough path — the compiler selects the correct impl
/// based on the inner service's request type.
#[derive(Clone)]
pub struct CompressionService<S> {
    inner: S,
    config: Arc<CompressionConfig>,
}

impl<S> CompressionService<S> {
    pub(crate) fn new(inner: S, config: Arc<CompressionConfig>) -> Self {
        Self { inner, config }
    }
}

impl<S> Service<CompletionRequest> for CompressionService<S>
where
    S: Service<CompletionRequest>,
{
    type Response = S::Response;
    type Error = S::Error;
    // Compression is synchronous — no extra future wrapping needed.
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: CompletionRequest) -> Self::Future {
        let compressed = {
            let _span = tracing::debug_span!(
                "gateway.compression",
                agent = ?self.config.agent,
            )
            .entered();
            compress_request(&self.config, req)
        };
        self.inner.call(compressed)
    }
}

impl<S> Service<PassthroughRequest> for CompressionService<S>
where
    S: Service<PassthroughRequest>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: PassthroughRequest) -> Self::Future {
        {
            let _span = tracing::debug_span!(
                "gateway.compression.passthrough",
                agent = ?self.config.agent,
            )
            .entered();
            crate::compress::passthrough::compress_passthrough_body(&self.config, &mut req.body);
        }
        self.inner.call(req)
    }
}
