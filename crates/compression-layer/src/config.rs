use std::sync::Arc;

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

impl AgentType {
    /// Built-in tool names that must never be pruned from a request's
    /// `tools` array. These are typically small and load-bearing.
    pub fn core_tools(&self) -> &'static [&'static str] {
        match self {
            // Conservative super-set of common Claude Code built-ins. Anything
            // not in this list and not matching the MCP naming convention is
            // still kept (the heuristic only prunes things that look like MCP).
            AgentType::Claude => &[
                "Bash",
                "Read",
                "Write",
                "Edit",
                "Glob",
                "Grep",
                "LS",
                "WebFetch",
                "WebSearch",
                "Task",
                "TodoWrite",
                "NotebookEdit",
                "BashOutput",
                "KillShell",
                "ExitPlanMode",
            ],
            AgentType::Codex => &[
                "shell_command",
                "exec_command",
                "read_file",
                "grep",
                "list_directory",
                "write_file",
                "apply_patch",
            ],
            AgentType::OpenCode => &[
                "bash",
                "read",
                "write",
                "edit",
                "grep",
                "glob",
                "list",
                "patch",
                "todoread",
                "todowrite",
                "webfetch",
            ],
        }
    }
}

/// Configuration for request-level tool-set pruning.
///
/// Pruning runs only when a request's serialized `tools` array exceeds
/// `threshold_bytes`, so small requests are never touched.
#[derive(Debug, Clone)]
pub struct ToolPruningConfig {
    /// Whether pruning is enabled at all.
    pub enabled: bool,
    /// Run pruning only when the serialized tools array exceeds this many bytes.
    pub threshold_bytes: usize,
    /// Minimum number of MCP tools to keep even if scoring rejects them all.
    pub min_kept: usize,
    /// Minimum lexical-overlap score for an MCP tool to survive.
    pub min_score: u32,
}

impl Default for ToolPruningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_bytes: 4096,
            min_kept: 3,
            min_score: 1,
        }
    }
}

/// Configuration for the compression layer.
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub agent: AgentType,
    pub tool_pruning: ToolPruningConfig,
}

impl CompressionConfig {
    pub fn new(agent: AgentType) -> Arc<Self> {
        Arc::new(Self {
            agent,
            tool_pruning: ToolPruningConfig::default(),
        })
    }
}
