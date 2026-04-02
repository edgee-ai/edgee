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
use crate::strategy::claude::read::{Language, filter_minimal};

/// Below this many content lines, don't compress at all.
const SMALL_THRESHOLD: usize = 50;

pub struct ReadCompressor;

impl ToolCompressor for ReadCompressor {
    fn compress(&self, _arguments: &str, output: &str) -> Option<String> {
        let file_path = extract_path(output);
        let raw_content = extract_content(output)?;
        let content = strip_line_numbers(&raw_content);
        let lines: Vec<&str> = content.lines().collect();

        if lines.len() < SMALL_THRESHOLD {
            return None;
        }

        let lang = file_path
            .as_deref()
            .and_then(|p| Path::new(p).extension())
            .and_then(|e| e.to_str())
            .map(Language::from_extension)
            .unwrap_or(Language::Unknown);

        // Aggressive mode disabled for now — only apply minimal filtering
        let compressed = filter_minimal(&content, &lang);

        if compressed.is_empty() {
            return None;
        }

        // Only return if we actually saved something meaningful (>10%)
        let threshold = content.len() * 9 / 10;
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

/// Strip OpenCode line number prefixes. Format: `N:content`.
fn strip_line_numbers(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    for line in content.lines() {
        if let Some(colon_pos) = line.find(':')
            && line[..colon_pos].trim().chars().all(|c| c.is_ascii_digit())
        {
            result.push_str(&line[colon_pos + 1..]);
            result.push('\n');
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_strip_line_numbers() {
        let input = "1:use std::io;\n2:\n3:fn main() {\n";
        let result = strip_line_numbers(input);
        assert_eq!(result, "use std::io;\n\nfn main() {\n");
    }

    #[test]
    fn test_strip_line_numbers_preserves_non_numbered() {
        let input = "not numbered\n1:numbered\n";
        let result = strip_line_numbers(input);
        assert_eq!(result, "not numbered\nnumbered\n");
    }

    #[test]
    fn test_strip_line_numbers_content_with_colons() {
        // Content itself contains colons — only the first colon is the separator
        let input = "10:http://example.com\n";
        let result = strip_line_numbers(input);
        assert_eq!(result, "http://example.com\n");
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
