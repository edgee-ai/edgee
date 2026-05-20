//! Tool-output compression strategies for AI coding agents.
//!
//! Provides compressors for tool outputs from Claude Code, OpenCode, and Codex agents.
//! Each compressor reduces token usage by summarizing tool results while preserving
//! critical information.
//!
//! # Overview
//!
//! This crate is a **pure library with no network I/O**. It receives raw tool-output
//! strings and returns optionally-compressed strings. Agent-specific dispatch and
//! per-command bash strategies are entirely self-contained; nothing outside this crate
//! needs to know which tool produced which output.
//!
//! # Entry point
//!
//! [`compress_tool_output`] is the main entry point for Claude Code callers:
//!
//! ```rust
//! use edgee_compressor::compress_tool_output;
//!
//! let result = compress_tool_output("Read", r#"{"file_path": "src/lib.rs"}"#, "...");
//! // Returns Some(compressed) or None if no compressor is registered for that tool.
//! ```
//!
//! # Agent dispatch
//!
//! Each coding agent uses different tool names. Use the appropriate lookup function:
//!
//! | Agent | Lookup | Example tool name |
//! |---|---|---|
//! | Claude Code | [`claude_compressor_for`] | `"Read"`, `"Bash"` |
//! | Codex | [`codex_compressor_for`] | `"read_file"`, `"shell_command"` |
//! | OpenCode | [`opencode_compressor_for`] | `"read"`, `"bash"` |
//!
//! # System-reminder protection
//!
//! Claude Code embeds `<system-reminder>` blocks in tool output. These blocks carry
//! injected instructions and must never be modified. All Claude-facing compression
//! helpers split output into compressible and protected segments, compress only the
//! compressible parts, and reassemble verbatim. See
//! [`compress_claude_tool_with_segment_protection`] and the [`util`] module.
//!
//! # Extending compression
//!
//! Implement [`ToolCompressor`] and register the new compressor in the appropriate
//! `compressor_for` dispatch function under `strategy/`. See `CONTRIBUTING.md` for
//! a step-by-step guide.

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
