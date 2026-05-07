use std::sync::Arc;

/// Which agent's tool-name conventions to use when dispatching compressors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    /// Claude Code — tool names: `Bash`, `Read`, `Grep`, `Glob`
    Claude,
    /// Codex CLI — tool names: `shell_command`, `read_file`, `grep`, `list_directory`
    Codex,
    /// Cursor — tool names: `Shell`, `read_file`, `grep_search`, `list_dir`
    Cursor,
    /// OpenCode — tool names: `bash`, `read`, `grep`, `glob`
    OpenCode,
}

/// Configuration for the compression layer.
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub agent: AgentType,
}

impl CompressionConfig {
    pub fn new(agent: AgentType) -> Arc<Self> {
        Arc::new(Self { agent })
    }
}
