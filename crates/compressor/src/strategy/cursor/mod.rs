//! Cursor tool output compressors.
//!
//! Cursor Agent uses the same compression logic as Claude Code, but with
//! different tool names and output formats. Tool name mapping:
//! - `Shell`       → `ShellCompressor` (strips cursor wrapper, then uses Bash compressor)
//! - `read_file`   → reuses the Claude `Read` compressor
//! - `grep_search` → reuses the Claude `Grep` compressor
//! - `list_dir`    → reuses the Claude `Glob` compressor

use super::ToolCompressor;

/// Select the appropriate compressor for a Cursor tool name.
/// Returns `None` for tools we don't compress.
pub fn compressor_for(tool_name: &str) -> Option<&'static dyn ToolCompressor> {
    match tool_name {
        "Shell" => Some(&ShellCompressor),
        "read_file" => super::claude::compressor_for("Read"),
        "grep_search" => super::claude::compressor_for("Grep"),
        "list_dir" => super::claude::compressor_for("Glob"),
        _ => None,
    }
}

/// Cursor shell output wraps the actual command output in a structured format:
///
/// ````text
/// Exit code: 0
///
/// Command output:
///
/// ```
/// <actual output>
/// ```
///
/// Command completed in N ms.
///
/// Shell state ...
///
/// SANDBOXING: ...
/// ````
///
/// This compressor strips that wrapper before delegating to the Bash compressor.
struct ShellCompressor;

impl ToolCompressor for ShellCompressor {
    fn compress(&self, arguments: &str, output: &str) -> Option<String> {
        let inner = extract_shell_output(output).unwrap_or(output);
        super::claude::compressor_for("Bash")?.compress(arguments, inner)
    }
}

/// Extract the actual command output from the cursor shell wrapper.
/// Returns `None` if the expected format is not found (caller falls back to raw output).
fn extract_shell_output(output: &str) -> Option<&str> {
    // Find the opening fence (``` on its own line)
    let fence_start = output.find("\n```\n")?;
    let inner_start = fence_start + 5; // skip "\n```\n"

    // Find the closing fence
    let fence_end = output[inner_start..].find("\n```")?;

    Some(output[inner_start..inner_start + fence_end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_for_shell() {
        assert!(compressor_for("Shell").is_some());
    }

    #[test]
    fn compressor_for_read_file() {
        assert!(compressor_for("read_file").is_some());
    }

    #[test]
    fn compressor_for_grep_search() {
        assert!(compressor_for("grep_search").is_some());
    }

    #[test]
    fn compressor_for_list_dir() {
        assert!(compressor_for("list_dir").is_some());
    }

    #[test]
    fn compressor_for_unknown_tool() {
        assert!(compressor_for("unknown").is_none());
    }

    #[test]
    fn extract_shell_output_strips_wrapper() {
        let output = "Exit code: 0\n\nCommand output:\n\n```\ntotal 68\ndrwxr-xr-x  9 root root 4096 Apr  1 09:51 .\n```\n\nCommand completed in 1172 ms.\n\nShell state (cwd, env vars) persists for subsequent calls.";
        assert_eq!(
            extract_shell_output(output),
            Some("total 68\ndrwxr-xr-x  9 root root 4096 Apr  1 09:51 .")
        );
    }

    #[test]
    fn extract_shell_output_no_fence_returns_none() {
        let output = "just raw output with no fences";
        assert_eq!(extract_shell_output(output), None);
    }
}
