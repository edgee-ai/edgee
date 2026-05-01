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
//! Non-zero exit codes are re-injected as a `[exit N]` prefix on the
//! compressed result so the agent does not lose the failure signal.

use super::ToolCompressor;
use crate::util::COMPRESSION_MARKER;

/// Compress a Codex tool output, stripping the Codex header before delegating
/// to the appropriate compressor. Non-zero exit codes are preserved as a
/// `[exit N]` prefix so the agent still sees that the command failed.
pub fn compress(tool_name: &str, arguments: &str, output: &str) -> Option<String> {
    let compressor = compressor_for(tool_name)?;

    // Capture the exit code from the header BEFORE stripping it.
    let exit_code = parse_exit_code(output);
    let stripped = strip_header(output);

    let compressed =
        crate::util::compress_claude_tool_with_segment_protection(compressor, arguments, stripped)?;

    // Insert `[exit N] ` immediately after the version marker for non-zero
    // exits. Placing it before the marker would break the idempotency check
    // (`starts_with("<!--ec")`), causing the next pass to re-compress.
    Some(match exit_code {
        Some(code) if code != 0 => {
            if let Some(rest) = compressed.strip_prefix(COMPRESSION_MARKER) {
                format!("{COMPRESSION_MARKER}[exit {code}] {rest}")
            } else {
                format!("[exit {code}] {compressed}")
            }
        }
        _ => compressed,
    })
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

/// Parse the exit code from the Codex header. Recognized forms:
/// - `Exit code: N`
/// - `Process exited with code N`
///
/// Returns `None` if no recognized line exists, the value isn't an integer,
/// or there's no header at all.
fn parse_exit_code(output: &str) -> Option<i32> {
    let header = if let Some(end) = output.find("\nOutput:\n") {
        &output[..end]
    } else {
        // No header marker — try the whole input anyway, the line scan is cheap.
        output
    };

    for line in header.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Exit code:") {
            return rest.trim().parse::<i32>().ok();
        }
        if let Some(rest) = trimmed.strip_prefix("Process exited with code") {
            return rest.trim().parse::<i32>().ok();
        }
    }
    None
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

    #[test]
    fn parse_exit_code_classic_format() {
        assert_eq!(parse_exit_code("Exit code: 0\nOutput:\nfoo"), Some(0));
        assert_eq!(parse_exit_code("Exit code: 1\nOutput:\nfoo"), Some(1));
        assert_eq!(parse_exit_code("Exit code: 137\nOutput:\nfoo"), Some(137));
    }

    #[test]
    fn parse_exit_code_process_exited_format() {
        let out = "Command: zsh\nProcess exited with code 2\nOutput:\nbody\n";
        assert_eq!(parse_exit_code(out), Some(2));
    }

    #[test]
    fn parse_exit_code_missing_returns_none() {
        assert_eq!(parse_exit_code("plain output\n"), None);
        assert_eq!(parse_exit_code("Wall time: 5s\nOutput:\nfoo"), None);
    }

    #[test]
    fn nonzero_exit_is_reinjected_after_marker() {
        // Big enough ls -la output so the compressor actually returns Some.
        let args = r#"{"command":"ls -la","workdir":"/tmp"}"#;
        let mut body = String::from("total 999\n");
        for i in 0..40 {
            body.push_str(&format!("-rw-r--r-- 1 u s 10 Jan 1 12:00 file{i}.txt\n"));
        }
        let output = format!("Exit code: 1\nWall time: 0 seconds\nOutput:\n{body}");

        let result = compress("shell_command", args, &output).expect("should compress");
        assert!(
            result.starts_with(crate::util::COMPRESSION_MARKER),
            "marker must lead so idempotency check still works"
        );
        assert!(
            result.contains("[exit 1]"),
            "non-zero exit code must be re-injected; got: {result}"
        );
    }

    #[test]
    fn zero_exit_is_not_injected() {
        let args = r#"{"command":"ls -la","workdir":"/tmp"}"#;
        let mut body = String::from("total 999\n");
        for i in 0..40 {
            body.push_str(&format!("-rw-r--r-- 1 u s 10 Jan 1 12:00 file{i}.txt\n"));
        }
        let output = format!("Exit code: 0\nWall time: 0 seconds\nOutput:\n{body}");

        let result = compress("shell_command", args, &output).expect("should compress");
        assert!(
            !result.contains("[exit "),
            "zero exit must not produce a prefix; got: {result}"
        );
    }

    #[test]
    fn idempotent_with_exit_prefix() {
        // Re-running compress on its own output must short-circuit.
        let args = r#"{"command":"ls -la","workdir":"/tmp"}"#;
        let mut body = String::from("total 999\n");
        for i in 0..40 {
            body.push_str(&format!("-rw-r--r-- 1 u s 10 Jan 1 12:00 file{i}.txt\n"));
        }
        let output = format!("Exit code: 1\nWall time: 0 seconds\nOutput:\n{body}");

        let first = compress("shell_command", args, &output).expect("first pass compresses");
        // Second pass: feed the compressed output back through.
        let second = compress("shell_command", args, &first);
        assert!(second.is_none(), "second pass must short-circuit");
    }
}
