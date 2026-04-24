pub mod bash;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode;
pub mod util;

/// Trait for compressing the output of a specific tool.
/// `arguments` is the raw JSON string from tool_call.function.arguments.
/// Returns `Some(compressed)` if compression was applied, `None` to leave as-is.
pub trait ToolCompressor {
    fn compress(&self, arguments: &str, output: &str) -> Option<String>;
}
