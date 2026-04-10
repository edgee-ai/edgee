//! Claude Code tool output compressors.
//!
//! Each Claude Code tool that can be compressed gets its own module
//! implementing the `ToolCompressor` trait.

mod bash;
mod glob;
mod grep;
pub(crate) mod read;

pub use super::ToolCompressor;

/// Select the appropriate compressor for a Claude Code tool name.
/// Returns `None` for tools we don't compress.
pub fn compressor_for(tool_name: &str) -> Option<&'static dyn ToolCompressor> {
    match tool_name {
        "Bash" => Some(&bash::BashCompressor),
        "Read" => Some(&read::ReadCompressor),
        "Grep" => Some(&grep::GrepCompressor),
        "Glob" => Some(&glob::GlobCompressor),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_for_bash() {
        assert!(compressor_for("Bash").is_some());
    }

    #[test]
    fn compressor_for_read() {
        assert!(compressor_for("Read").is_some());
    }

    #[test]
    fn compressor_for_grep() {
        assert!(compressor_for("Grep").is_some());
    }

    #[test]
    fn compressor_for_glob() {
        assert!(compressor_for("Glob").is_some());
    }

    #[test]
    fn compressor_for_unknown_tool() {
        assert!(compressor_for("Unknown").is_none());
    }

    #[test]
    fn compressor_for_empty_string() {
        assert!(compressor_for("").is_none());
    }

    #[test]
    fn compressor_for_case_sensitive() {
        // Tool names are case-sensitive — "bash" (lowercase) is not a known tool
        assert!(compressor_for("bash").is_none());
        assert!(compressor_for("read").is_none());
    }
}
