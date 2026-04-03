//! Compressor for the OpenCode `read` tool output.
//!
//! OpenCode's read tool outputs content in a different format than Claude Code:
//!
//! ```text
//! <path>filepath</path>
//! <type>file</type>
//! <content>1:line content
//! 2:more content
//! </content>
//! ```
//!
//! This compressor extracts the file path and content, strips the `N:` line
//! number prefixes, then applies the same language-aware filtering as the
//! Claude Code Read compressor.

use std::path::Path;

use crate::strategy::ToolCompressor;
use crate::strategy::claude::read::{
    Language, filter_minimal_numbered, format_numbered_lines, parse_numbered_lines,
};

/// Below this many content lines, don't compress at all.
const SMALL_THRESHOLD: usize = 50;

pub struct ReadCompressor;

impl ToolCompressor for ReadCompressor {
    fn compress(&self, _arguments: &str, output: &str) -> Option<String> {
        let file_path = extract_path(output);
        let raw_content = extract_content(output)?;
        let (fmt, numbered) = parse_numbered_lines(&raw_content);

        if numbered.len() < SMALL_THRESHOLD {
            return None;
        }

        let lang = file_path
            .as_deref()
            .and_then(|p| Path::new(p).extension())
            .and_then(|e| e.to_str())
            .map(Language::from_extension)
            .unwrap_or(Language::Unknown);

        let filtered = filter_minimal_numbered(&numbered, &lang);

        if filtered.is_empty() {
            return None;
        }

        let compressed = format_numbered_lines(&filtered, fmt);

        // Only return if we actually saved something meaningful (>10%)
        let threshold = raw_content.len() * 9 / 10;
        if compressed.len() >= threshold {
            return None;
        }

        Some(compressed)
    }
}

/// Extract the file path from `<path>...</path>` in the output.
fn extract_path(output: &str) -> Option<String> {
    let start = output.find("<path>")? + "<path>".len();
    let end = output[start..].find("</path>")? + start;
    Some(output[start..end].trim().to_string())
}

/// Extract the content from `<content>...</content>` in the output.
fn extract_content(output: &str) -> Option<String> {
    let start = output.find("<content>")? + "<content>".len();
    let end = output[start..].find("</content>")? + start;
    Some(output[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::claude::read::LineFormat;

    fn make_output(path: &str, lines: usize) -> String {
        let mut content = String::new();
        content.push_str(&format!("<path>{}</path>\n", path));
        content.push_str("<type>file</type>\n");
        content.push_str("<content>");
        // 4 header lines + body
        content.push_str("1:use std::io;\n");
        content.push_str("2:\n");
        content.push_str("3:// This is a comment\n");
        content.push_str("4:/// Doc comment\n");
        content.push_str("5:fn main() {\n");
        let mut ln = 6;
        for _ in 0..lines {
            content.push_str(&format!("{}:    println!(\"hello\");\n", ln));
            ln += 1;
            content.push_str(&format!("{}:    // TODO: refactor\n", ln));
            ln += 1;
        }
        content.push_str(&format!("{}:{{}}\n", ln));
        content.push_str("</content>");
        content
    }

    #[test]
    fn test_extract_path() {
        let output = "<path>/src/main.rs</path>\n<type>file</type>\n<content>1:hello</content>";
        assert_eq!(extract_path(output), Some("/src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_path_missing() {
        assert_eq!(extract_path("<content>1:hello</content>"), None);
    }

    #[test]
    fn test_extract_content() {
        let output = "<path>/src/main.rs</path>\n<content>1:hello\n2:world\n</content>";
        assert_eq!(
            extract_content(output),
            Some("1:hello\n2:world\n".to_string())
        );
    }

    #[test]
    fn test_extract_content_missing() {
        assert_eq!(extract_content("<path>/src/main.rs</path>"), None);
    }

    #[test]
    fn test_parse_numbered_lines() {
        let input = "1:use std::io;\n2:\n3:fn main() {\n";
        let (fmt, result) = parse_numbered_lines(input);
        assert_eq!(fmt, LineFormat::Colon);
        assert_eq!(result[0], (Some(1), "use std::io;".to_string()));
        assert_eq!(result[1], (Some(2), "".to_string()));
        assert_eq!(result[2], (Some(3), "fn main() {".to_string()));
    }

    #[test]
    fn test_parse_numbered_lines_non_numbered() {
        let input = "not numbered\n1:numbered\n";
        let (_, result) = parse_numbered_lines(input);
        assert_eq!(result[0], (None, "not numbered".to_string()));
        assert_eq!(result[1], (Some(1), "numbered".to_string()));
    }

    #[test]
    fn test_parse_numbered_lines_content_with_colons() {
        let input = "10:http://example.com\n";
        let (_, result) = parse_numbered_lines(input);
        assert_eq!(result[0], (Some(10), "http://example.com".to_string()));
    }

    #[test]
    fn test_compressed_output_preserves_line_numbers() {
        let output = make_output("/src/main.rs", 60);
        let compressor = ReadCompressor;
        let compressed = compressor.compress("{}", &output).unwrap();
        // Line 3 (comment) is stripped; line 4 (doc comment) should keep number 4.
        assert!(
            compressed.contains("4:/// Doc comment"),
            "doc comment should keep line number 4"
        );
        assert!(
            compressed.contains("1:use std::io;"),
            "import should keep line number 1"
        );
        // Stripped comment must not appear
        assert!(!compressed.contains("// This is a comment"));
    }

    #[test]
    fn test_small_output_not_compressed() {
        let output = make_output("/src/main.rs", 10);
        let compressor = ReadCompressor;
        assert!(compressor.compress("{}", &output).is_none());
    }

    #[test]
    fn test_strips_comments_rust() {
        let output = make_output("/src/main.rs", 60);
        let compressor = ReadCompressor;
        let result = compressor.compress("{}", &output);
        assert!(result.is_some());
        let compressed = result.unwrap();
        assert!(!compressed.contains("// This is a comment"));
        assert!(compressed.contains("/// Doc comment"));
        assert!(compressed.contains("use std::io;"));
        assert!(compressed.contains("fn main()"));
    }

    #[test]
    fn test_no_content_tag_returns_none() {
        let output = "<path>/src/main.rs</path>\n<type>file</type>\nsome raw content";
        let compressor = ReadCompressor;
        assert!(compressor.compress("{}", output).is_none());
    }
}
