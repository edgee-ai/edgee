use std::sync::Arc;

use crate::{config::CompressionConfig, service::CompressionService};

/// Tower [`Layer`](tower::Layer) that wraps a downstream service with tool-result compression.
///
/// Construct via [`CompressionLayer::new`], then compose with Tower's
/// [`ServiceBuilder`](tower::ServiceBuilder):
///
/// ```rust,ignore
/// let svc = ServiceBuilder::new()
///     .layer(CompressionLayer::new(CompressionConfig::new(AgentType::Claude)))
///     .service(dispatch_service);
/// ```
#[derive(Clone)]
pub struct CompressionLayer {
    config: Arc<CompressionConfig>,
}

impl CompressionLayer {
    pub fn new(config: impl Into<Arc<CompressionConfig>>) -> Self {
        Self {
            config: config.into(),
        }
    }
}

impl<S> tower::Layer<S> for CompressionLayer {
    type Service = CompressionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CompressionService::new(inner, Arc::clone(&self.config))
    }
}
