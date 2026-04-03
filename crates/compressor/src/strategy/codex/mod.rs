//! Codex CLI tool output compressors.
//!
//! Codex CLI uses the same compression logic as Claude Code, but with
//! different tool names. Tool name mapping:
//! - `shell_command` → reuses the Claude `Bash` compressor
//! - `read_file`     → reuses the Claude `Read` compressor
//! - `grep`          → reuses the Claude `Grep` compressor
//! - `list_directory` → reuses the Claude `Glob` compressor
//!
//! Codex tool outputs are prefixed with a header block:
//!   Exit code: N\nWall time: N seconds\nOutput:\n
//! This is stripped before compression so compressors see only the raw output.

use super::ToolCompressor;

/// Compress a Codex tool output, stripping the Codex header before delegating
/// to the appropriate compressor.
pub fn compress(tool_name: &str, arguments: &str, output: &str) -> Option<String> {
    let compressor = compressor_for(tool_name)?;
    let stripped = strip_header(output);

    crate::util::compress_claude_tool_with_segment_protection(compressor, arguments, stripped)
}

/// Strip the Codex shell output header, which always ends with "\nOutput:\n".
///
/// Everything up to and including the first "\nOutput:\n" is treated as
/// header metadata and discarded. If the marker is not present the output
/// is returned unchanged.
fn strip_header(output: &str) -> &str {
    if let Some(pos) = output.find("\nOutput:\n") {
        &output[pos + "\nOutput:\n".len()..]
    } else {
        output
    }
}

/// Select the appropriate compressor for a Codex CLI tool name.
/// Returns `None` for tools we don't compress.
pub fn compressor_for(tool_name: &str) -> Option<&'static dyn ToolCompressor> {
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

    #[test]
    fn strip_header_new_format() {
        let output = "Command: zsh -lc 'ls'\nChunk ID: abc123\nWall time: 0.0000 seconds\nProcess exited with code 0\nOriginal token count: 42\nOutput:\nhello\nworld\n";
        assert_eq!(strip_header(output), "hello\nworld\n");
    }

    #[test]
    fn strip_header_full() {
        let output = "Exit code: 0\nWall time: 0 seconds\nOutput:\nhello\nworld\n";
        assert_eq!(strip_header(output), "hello\nworld\n");
    }

    #[test]
    fn strip_header_partial() {
        let output = "Exit code: 1\nOutput:\nerror text\n";
        assert_eq!(strip_header(output), "error text\n");
    }

    #[test]
    fn strip_header_none() {
        let output = "plain output\n";
        assert_eq!(strip_header(output), "plain output\n");
    }

    #[test]
    fn strip_header_output_at_start_of_content() {
        // Starts with "Output:\n" but no preceding "\n" — not a header boundary
        let output = "Output:\nsome content\n";
        assert_eq!(strip_header(output), "Output:\nsome content\n");
    }

    #[test]
    fn compress_strips_header_before_compressing() {
        // ls -la output wrapped in Codex header — should compress successfully
        let args = r#"{"command":"ls -la","workdir":"/tmp"}"#;
        let output = "Exit code: 0\nWall time: 0 seconds\nOutput:\ntotal 8\ndrwxr-xr-x 2 user staff 64 Jan 1 12:00 .\ndrwxr-xr-x 2 user staff 64 Jan 1 12:00 ..\n-rw-r--r-- 1 user staff 10 Jan 1 12:00 file.txt\n";
        assert!(compress("shell_command", args, output).is_some());
    }
}
