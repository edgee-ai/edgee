//! Tool output compression strategies for AI coding agents.
//!
//! Provides compressors for tool outputs from Claude Code, OpenCode, and Codex agents.
//! Each compressor reduces token usage by summarizing tool results while preserving
//! critical information.

pub mod strategy;
pub mod util;

// Re-export key traits
pub use strategy::ToolCompressor;
pub use strategy::bash::BashCompressor;

// Re-export compressor lookup functions
pub use strategy::bash::compressor_for as bash_compressor_for;
pub use strategy::claude::compressor_for as claude_compressor_for;
pub use strategy::codex::compressor_for as codex_compressor_for;
pub use strategy::opencode::compressor_for as opencode_compressor_for;

// Re-export complete compression pipelines (includes header stripping, segment protection, etc.)
pub use strategy::codex::compress as compress_codex_tool_output;

// Re-export the main compression utility
pub use util::compress_claude_tool_with_segment_protection;

/// Compress a Claude Code tool output by tool name.
///
/// Looks up the appropriate compressor for the given tool name and applies it,
/// preserving `<system-reminder>` blocks verbatim.
///
/// Returns `Some(compressed)` if compression was applied, `None` to keep the original.
pub fn compress_tool_output(tool_name: &str, arguments: &str, output: &str) -> Option<String> {
    let compressor = claude_compressor_for(tool_name)?;
    compress_claude_tool_with_segment_protection(compressor, arguments, output)
}
