//! OpenCode tool output compressors.
//!
//! OpenCode uses the same compression logic as Claude Code, but with lowercase
//! tool names. The `read` tool has a different output format (XML-wrapped with
//! `N:` line number prefixes) and uses its own compressor.

mod read;

use super::claude::ClaudeToolCompressor;

/// Select the appropriate compressor for an OpenCode tool name.
/// Returns `None` for tools we don't compress.
pub fn compressor_for(tool_name: &str) -> Option<&'static dyn ClaudeToolCompressor> {
    match tool_name {
        "bash" => super::claude::compressor_for("Bash"),
        "read" => Some(&read::ReadCompressor),
        "grep" => super::claude::compressor_for("Grep"),
        "glob" => super::claude::compressor_for("Glob"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_for_bash() {
        assert!(compressor_for("bash").is_some());
    }

    #[test]
    fn compressor_for_read() {
        assert!(compressor_for("read").is_some());
    }

    #[test]
    fn compressor_for_grep() {
        assert!(compressor_for("grep").is_some());
    }

    #[test]
    fn compressor_for_glob() {
        assert!(compressor_for("glob").is_some());
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
        // Tool names are case-sensitive — "Bash" (PascalCase) is not an OpenCode tool
        assert!(compressor_for("Bash").is_none());
        assert!(compressor_for("Read").is_none());
    }
}
