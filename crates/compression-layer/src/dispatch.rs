//! Shared helper that routes a tool name + arguments + text to the right
//! per-agent compressor. Lives in a standalone module so both compression
//! paths (typed `CompletionRequest` and JSON passthrough) can call it without
//! either path appearing to "own" the other.

use crate::config::{AgentType, CompressionConfig};

/// Dispatch to the per-agent compressor and return the compressed text, or
/// `None` if no compressor applied.
pub(crate) fn compress_with_agent(
    config: &CompressionConfig,
    name: &str,
    arguments: &str,
    text: &str,
) -> Option<String> {
    match config.agent {
        AgentType::Codex => edgee_compressor::compress_codex_tool_output(name, arguments, text),
        AgentType::Claude => edgee_compressor::claude_compressor_for(name).and_then(|c| {
            edgee_compressor::compress_claude_tool_with_segment_protection(c, arguments, text)
        }),
        AgentType::OpenCode => edgee_compressor::opencode_compressor_for(name).and_then(|c| {
            edgee_compressor::compress_claude_tool_with_segment_protection(c, arguments, text)
        }),
    }
}
