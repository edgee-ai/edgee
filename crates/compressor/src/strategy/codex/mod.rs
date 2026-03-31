//! Codex CLI tool output compressors.
//!
//! Codex CLI uses the same compression logic as Claude Code, but with
//! different tool names. Tool name mapping:
//! - `shell_command` → reuses the Claude `Bash` compressor
//! - `read_file`     → reuses the Claude `Read` compressor
//! - `grep`          → reuses the Claude `Grep` compressor
//! - `list_directory` → reuses the Claude `Glob` compressor

use super::claude::ClaudeToolCompressor;

/// Select the appropriate compressor for a Codex CLI tool name.
/// Returns `None` for tools we don't compress.
pub fn compressor_for(tool_name: &str) -> Option<&'static dyn ClaudeToolCompressor> {
    match tool_name {
        "exec_command" => super::claude::compressor_for("Bash"),
        "shell_command" => super::claude::compressor_for("Bash"),
        "read_file" => super::claude::compressor_for("Read"),
        "grep" => super::claude::compressor_for("Grep"),
        "list_directory" => super::claude::compressor_for("Glob"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_for_shell_command() {
        assert!(compressor_for("shell_command").is_some());
    }

    #[test]
    fn compressor_for_read_file() {
        assert!(compressor_for("read_file").is_some());
    }

    #[test]
    fn compressor_for_grep() {
        assert!(compressor_for("grep").is_some());
    }

    #[test]
    fn compressor_for_list_directory() {
        assert!(compressor_for("list_directory").is_some());
    }

    #[test]
    fn compressor_for_unknown_tool() {
        assert!(compressor_for("unknown").is_none());
    }

    #[test]
    fn compressor_for_empty_string() {
        assert!(compressor_for("").is_none());
    }

    #[test]
    fn compressor_for_case_sensitive() {
        assert!(compressor_for("Shell_Command").is_none());
        assert!(compressor_for("shell").is_none());
    }
}
