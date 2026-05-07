use std::sync::Arc;

use crate::{
    config::CompressionConfig,
    service::CompressionService,
    technique::{CompressionPipeline, ToolResultsTechnique},
};

/// Tower [`Layer`] that wraps a downstream service with the configured
/// compression pipeline.
///
/// Two construction paths:
///
/// - [`CompressionLayer::new`]: build a default single-technique pipeline
///   (`tool-results`) from a [`CompressionConfig`]. This is the path the
///   gateway uses today and stays drop-in compatible with previous releases.
/// - [`CompressionLayer::with_pipeline`]: install a fully custom pipeline.
///   Use this once we add more techniques (image down-sample, system-prompt
///   dedup, …) and you want to choose which ones run.
///
/// ```rust,ignore
/// let svc = ServiceBuilder::new()
///     .layer(CompressionLayer::new(CompressionConfig::new(AgentType::Claude)))
///     .service(dispatch_service);
/// ```
#[derive(Clone)]
pub struct CompressionLayer {
    pipeline: Arc<CompressionPipeline>,
}

impl CompressionLayer {
    /// Build a layer with the default pipeline = `[tool-results]`.
    pub fn new(config: impl Into<Arc<CompressionConfig>>) -> Self {
        let config = config.into();
        let pipeline = CompressionPipeline::new().with(ToolResultsTechnique::new(config));
        Self {
            pipeline: Arc::new(pipeline),
        }
    }

    /// Build a layer from a pre-assembled pipeline.
    pub fn with_pipeline(pipeline: impl Into<Arc<CompressionPipeline>>) -> Self {
        Self {
            pipeline: pipeline.into(),
        }
    }
}

impl<S> tower::Layer<S> for CompressionLayer {
    type Service = CompressionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CompressionService::new(inner, Arc::clone(&self.pipeline))
    }
}
