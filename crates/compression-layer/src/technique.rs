//! Composable compression pipeline.
//!
//! A [`CompressionTechnique`] is one step that mutates a [`CompletionRequest`].
//! Today the only built-in technique is [`ToolResultsTechnique`] (the existing
//! tool-output compression), but the design lets us add image down-sampling,
//! system-prompt deduplication, conversation summarization, etc. in the future
//! without touching the dispatch layer.
//!
//! A [`CompressionPipeline`] is just an ordered list of techniques. Each
//! technique sees the output of the previous one, so order matters when they
//! interact (e.g. de-dup before summarize).

use std::sync::Arc;

use edgee_ai_gateway_core::CompletionRequest;

use crate::compress::compress_request;
use crate::config::CompressionConfig;

/// One step in a [`CompressionPipeline`].
pub trait CompressionTechnique: Send + Sync {
    /// Stable identifier (used for metrics labels and debugging).
    fn name(&self) -> &'static str;

    /// Mutate `req` and return it. Implementations should be deterministic so
    /// the upstream prompt cache stays stable across retries.
    fn apply(&self, req: CompletionRequest) -> CompletionRequest;
}

/// Ordered list of techniques applied in sequence to every request.
///
/// Cheap to clone — the Vec lives inside the pipeline, the layer holds an
/// [`Arc`] around the whole struct so all cloned services share the same
/// instance.
#[derive(Default)]
pub struct CompressionPipeline {
    techniques: Vec<Box<dyn CompressionTechnique>>,
}

impl CompressionPipeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `technique` at the end of the pipeline.
    pub fn with(mut self, technique: impl CompressionTechnique + 'static) -> Self {
        self.techniques.push(Box::new(technique));
        self
    }

    /// Append a pre-boxed technique (escape hatch for runtime composition).
    pub fn push(&mut self, technique: Box<dyn CompressionTechnique>) {
        self.techniques.push(technique);
    }

    /// Run every technique in order on `req`.
    pub fn apply(&self, mut req: CompletionRequest) -> CompletionRequest {
        for t in &self.techniques {
            req = t.apply(req);
        }
        req
    }

    /// Names of the techniques, in execution order.
    pub fn names(&self) -> Vec<&'static str> {
        self.techniques.iter().map(|t| t.name()).collect()
    }

    pub fn len(&self) -> usize {
        self.techniques.len()
    }

    pub fn is_empty(&self) -> bool {
        self.techniques.is_empty()
    }
}

impl std::fmt::Debug for CompressionPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompressionPipeline")
            .field("techniques", &self.names())
            .finish()
    }
}

/// Tool-output compression — the only technique shipped today.
///
/// Wraps the existing [`compress_request`] entry point so the rest of the
/// compressor crate keeps its current public API.
pub struct ToolResultsTechnique {
    config: Arc<CompressionConfig>,
}

impl ToolResultsTechnique {
    pub fn new(config: Arc<CompressionConfig>) -> Self {
        Self { config }
    }
}

impl CompressionTechnique for ToolResultsTechnique {
    fn name(&self) -> &'static str {
        "tool-results"
    }

    fn apply(&self, req: CompletionRequest) -> CompletionRequest {
        compress_request(&self.config, req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgee_ai_gateway_core::types::{Message, MessageContent, UserMessage};

    /// A test technique that records its invocation by tagging the first
    /// user message with a marker.
    struct TagUserMessage(&'static str);

    impl CompressionTechnique for TagUserMessage {
        fn name(&self) -> &'static str {
            "tag-user"
        }
        fn apply(&self, mut req: CompletionRequest) -> CompletionRequest {
            for msg in &mut req.messages {
                if let Message::User(u) = msg {
                    let prev = u.content.as_text();
                    u.content = MessageContent::Text(format!("{}{}", self.0, prev));
                }
            }
            req
        }
    }

    fn empty_request() -> CompletionRequest {
        CompletionRequest::new(
            "test-model".to_string(),
            vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("hello".into()),
                cache_control: None,
            })],
        )
    }

    #[test]
    fn empty_pipeline_is_identity() {
        let p = CompressionPipeline::new();
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
        assert!(p.names().is_empty());
        let req = empty_request();
        let out = p.apply(req);
        assert_eq!(out.messages.len(), 1);
    }

    #[test]
    fn pipeline_runs_techniques_in_order() {
        // First technique prepends "[1]", second prepends "[2]".
        // After both, the user message starts with "[2][1]" — proving order.
        let p = CompressionPipeline::new()
            .with(TagUserMessage("[1]"))
            .with(TagUserMessage("[2]"));
        assert_eq!(p.names(), vec!["tag-user", "tag-user"]);
        let req = empty_request();
        let out = p.apply(req);
        let Message::User(u) = &out.messages[0] else {
            panic!("expected user message");
        };
        assert_eq!(u.content.as_text(), "[2][1]hello");
    }

    #[test]
    fn tool_results_technique_uses_shared_config() {
        // Two pipelines pointing at the same config share metrics.
        let cfg = CompressionConfig::new(crate::AgentType::Claude);
        let p1 = CompressionPipeline::new().with(ToolResultsTechnique::new(cfg.clone()));
        let p2 = CompressionPipeline::new().with(ToolResultsTechnique::new(cfg.clone()));
        // No-op requests — just verify the pipelines build and run.
        let _ = p1.apply(empty_request());
        let _ = p2.apply(empty_request());
        // Metrics totals stay at zero because there are no Tool messages.
        assert_eq!(cfg.metrics.totals().invocations, 0);
    }
}
