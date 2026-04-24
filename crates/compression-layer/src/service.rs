use std::{
    sync::Arc,
    task::{Context, Poll},
};

use edgee_gateway_core::CompletionRequest;
use tower::Service;

use crate::{compress::compress_request, config::CompressionConfig};

/// Tower [`Service`] produced by [`CompressionLayer`](crate::CompressionLayer).
///
/// Intercepts each [`CompletionRequest`], compresses tool-result content
/// in-place, then delegates to the wrapped inner service.
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
        let compressed = compress_request(&self.config, req);
        self.inner.call(compressed)
    }
}
