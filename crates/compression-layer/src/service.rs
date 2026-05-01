use std::{
    sync::Arc,
    task::{Context, Poll},
};

use edgee_ai_gateway_core::CompletionRequest;
use tower::Service;

use crate::technique::CompressionPipeline;

/// Tower [`Service`] produced by [`CompressionLayer`](crate::CompressionLayer).
///
/// Each request runs through the configured [`CompressionPipeline`] before it
/// reaches the wrapped inner service. The pipeline is shared (`Arc`) so cloned
/// services stay coherent.
pub struct CompressionService<S> {
    inner: S,
    pipeline: Arc<CompressionPipeline>,
}

impl<S> CompressionService<S> {
    pub(crate) fn new(inner: S, pipeline: Arc<CompressionPipeline>) -> Self {
        Self { inner, pipeline }
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
        let req = self.pipeline.apply(req);
        self.inner.call(req)
    }
}
