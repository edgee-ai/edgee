use std::sync::Arc;

use crate::metrics::CompressionMetrics;

/// Which agent's tool-name conventions to use when dispatching compressors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    /// Claude Code — tool names: `Bash`, `Read`, `Grep`, `Glob`
    Claude,
    /// Codex CLI — tool names: `shell_command`, `read_file`, `grep`, `list_directory`
    Codex,
    /// OpenCode — tool names: `bash`, `read`, `grep`, `glob`
    OpenCode,
}

/// Configuration for the compression layer.
///
/// `metrics` is shared with every cloned [`crate::CompressionLayer`] /
/// [`crate::CompressionService`] handle pointing at this config, so the
/// counters stay coherent across the whole gateway. Pass the same
/// `Arc<CompressionMetrics>` to [`Self::with_metrics`] when you want to
/// scrape it from another part of the application (e.g. a `/metrics`
/// HTTP handler).
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub agent: AgentType,
    pub metrics: Arc<CompressionMetrics>,
}

impl CompressionConfig {
    /// Build a config with a fresh, private metrics collector.
    pub fn new(agent: AgentType) -> Arc<Self> {
        Arc::new(Self {
            agent,
            metrics: Arc::new(CompressionMetrics::new()),
        })
    }

    /// Build a config that shares an externally owned metrics collector.
    pub fn with_metrics(agent: AgentType, metrics: Arc<CompressionMetrics>) -> Arc<Self> {
        Arc::new(Self { agent, metrics })
    }
}
