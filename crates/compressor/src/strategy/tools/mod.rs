//! Tool-set pruning: a request-level compression strategy that drops tool
//! definitions unlikely to be needed for the current turn.
//!
//! Unlike [`crate::ToolCompressor`] (which compresses a single tool's *output*),
//! a [`ToolSetCompressor`] operates on the request's whole *tools* array
//! before the request is forwarded to the provider. The motivating use case is
//! pruning bulky MCP-server tool definitions that the user's current turn has
//! no use for, while keeping the agent's built-in tools (Bash, Read, Grep, …)
//! and any MCP tool the agent has already invoked in this conversation.

pub mod heuristic;
pub mod tokenize;

pub use heuristic::HeuristicToolSetCompressor;

/// Read-only view of a request, sufficient to decide which tools to keep.
///
/// The borrows are short-lived: the caller (compression-layer) builds this
/// from the in-flight `CompletionRequest` and discards it after a single call.
pub struct PruneContext<'a> {
    /// All tool definitions on the request (in original order).
    pub tools: &'a [ToolView<'a>],
    /// Latest user-message text, if any.
    pub latest_user_text: Option<&'a str>,
    /// Names of every tool the assistant has already invoked in this conversation.
    /// Tools whose name is in this set are kept (sticky).
    pub prior_tool_call_names: &'a [&'a str],
    /// The list of "core" tool names that must always be kept regardless of score.
    pub core_tools: &'a [&'a str],
}

/// Projection of a single tool definition the pruner needs to score it.
///
/// Lets callers in `compression-layer` (which has `Vec<Tool>`) build the
/// context without forcing this crate to depend on `edgee-ai-gateway-core`.
#[derive(Debug, Clone, Copy)]
pub struct ToolView<'a> {
    /// Tool function name (`None` for opaque non-function tools — always kept).
    pub name: Option<&'a str>,
    /// Tool description, if any.
    pub description: Option<&'a str>,
    /// Approximate serialized size in bytes, used to estimate savings.
    pub size_bytes: usize,
}

/// The outcome of a pruning pass.
#[derive(Debug, Default, Clone)]
pub struct PruneDecision {
    /// Sorted indices (into the original `tools` slice) of tools to keep.
    pub keep_indices: Vec<usize>,
    /// Total bytes represented by the input tool set.
    pub bytes_before: usize,
    /// Total bytes represented by the kept tool set.
    pub bytes_after: usize,
    /// Number of tools dropped.
    pub dropped: usize,
}

impl PruneDecision {
    /// Identity decision: keep everything.
    pub fn keep_all(tools: &[ToolView<'_>]) -> Self {
        let bytes: usize = tools.iter().map(|t| t.size_bytes).sum();
        Self {
            keep_indices: (0..tools.len()).collect(),
            bytes_before: bytes,
            bytes_after: bytes,
            dropped: 0,
        }
    }
}

/// Trait for strategies that decide which tools to keep on a request.
pub trait ToolSetCompressor {
    fn prune(&self, ctx: &PruneContext<'_>) -> PruneDecision;
}
