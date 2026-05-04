use std::{
    sync::Arc,
    task::{Context, Poll},
};

use edgee_gateway_core::PassthroughRequest;
use tower::Service;

use crate::config::CompressionConfig;

mod compress;

/// Tower [`Layer`](tower::Layer) that wraps a passthrough service with
/// [`PassthroughCompressionService`], compressing tool-result content in
/// provider-native Anthropic JSON bodies before they reach the inner service.
#[derive(Clone)]
pub struct PassthroughCompressionLayer {
    config: Arc<CompressionConfig>,
}

impl PassthroughCompressionLayer {
    pub fn new(config: impl Into<Arc<CompressionConfig>>) -> Self {
        Self {
            config: config.into(),
        }
    }
}

impl<S> tower::Layer<S> for PassthroughCompressionLayer {
    type Service = PassthroughCompressionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PassthroughCompressionService::new(inner, Arc::clone(&self.config))
    }
}

/// Tower [`Service`] produced by [`PassthroughCompressionLayer`].
///
/// Intercepts each [`PassthroughRequest`], compresses tool-result content in
/// the provider-native JSON body in-place, then delegates to the wrapped inner service.
///
/// Compression is synchronous — no extra future wrapping is needed, so
/// `type Future = S::Future`.
#[derive(Clone)]
pub struct PassthroughCompressionService<S> {
    inner: S,
    config: Arc<CompressionConfig>,
}

impl<S> PassthroughCompressionService<S> {
    pub(crate) fn new(inner: S, config: Arc<CompressionConfig>) -> Self {
        Self { inner, config }
    }
}

impl<S> Service<PassthroughRequest> for PassthroughCompressionService<S>
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
            compress::compress_passthrough_body(&self.config, &mut req.body);
        }
        self.inner.call(req)
    }
}
