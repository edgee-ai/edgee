// Copyright 2024 rtk-ai and rtk-ai Labs
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Original source: https://github.com/rtk-ai/rtk
//
// Modifications copyright 2026 Edgee Cloud
// This file has been modified from its original form:
//   - Adapted from a local CLI proxy to a server-side gateway compressor
//   - Refactored to implement Edgee's traits
//   - Further adapted as needed for this module's role in the gateway
//
// See LICENSE-APACHE in the project root for the full license text.

//! Compressor for the Claude Code `Read` tool output.
//!
//! Read tool returns `cat -n` formatted file content with line numbers
//! (`     1\tcontent`). This compressor detects the language from the
//! file path, then applies RTK-style filtering: stripping comments,
//! collapsing blank lines, and optionally collapsing function bodies.

use std::path::Path;

use lazy_static::lazy_static;
use regex::Regex;

use super::ToolCompressor;

/// Below this many content lines, don't compress at all.
const SMALL_THRESHOLD: usize = 50;

// --- Language detection ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    C,
    Cpp,
    Java,
    Ruby,
    Shell,
    Unknown,
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => Language::Rust,
            "py" | "pyw" => Language::Python,
            "js" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "tsx" => Language::TypeScript,
            "go" => Language::Go,
            "c" | "h" => Language::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" => Language::Cpp,
            "java" => Language::Java,
            "rb" => Language::Ruby,
            "sh" | "bash" | "zsh" => Language::Shell,
            _ => Language::Unknown,
        }
    }

    pub fn comment_patterns(&self) -> CommentPatterns {
        match self {
            Language::Rust => CommentPatterns {
                line: Some("//"),
                block_start: Some("/*"),
                block_end: Some("*/"),
                doc_line: Some("///"),
                doc_block_start: Some("/**"),
            },
            Language::Python => CommentPatterns {
                line: Some("#"),
                block_start: Some("\"\"\""),
                block_end: Some("\"\"\""),
                doc_line: None,
                doc_block_start: Some("\"\"\""),
            },
            Language::JavaScript
            | Language::TypeScript
            | Language::Go
            | Language::C
            | Language::Cpp
            | Language::Java => CommentPatterns {
                line: Some("//"),
                block_start: Some("/*"),
                block_end: Some("*/"),
                doc_line: None,
                doc_block_start: Some("/**"),
            },
            Language::Ruby => CommentPatterns {
                line: Some("#"),
                block_start: Some("=begin"),
                block_end: Some("=end"),
                doc_line: None,
                doc_block_start: None,
            },
            Language::Shell => CommentPatterns {
                line: Some("#"),
                block_start: None,
                block_end: None,
                doc_line: None,
                doc_block_start: None,
            },
            Language::Unknown => CommentPatterns {
                line: Some("//"),
                block_start: Some("/*"),
                block_end: Some("*/"),
                doc_line: None,
                doc_block_start: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommentPatterns {
    pub line: Option<&'static str>,
    pub block_start: Option<&'static str>,
    pub block_end: Option<&'static str>,
    pub doc_line: Option<&'static str>,
    pub doc_block_start: Option<&'static str>,
}

// --- Filters ---

lazy_static! {
    static ref MULTIPLE_BLANK_LINES: Regex = Regex::new(r"\n{3,}").unwrap();
    static ref IMPORT_PATTERN: Regex =
        Regex::new(r"^(use |import |from |require\(|#include)").unwrap();
    static ref FUNC_SIGNATURE: Regex = Regex::new(
        r"^(pub\s+)?(async\s+)?(fn|def|function|func|class|struct|enum|trait|interface|type)\s+\w+"
    )
    .unwrap();
}

/// Strip comments while preserving doc comments and collapse blank lines.
pub(crate) fn filter_minimal(content: &str, lang: &Language) -> String {
    let patterns = lang.comment_patterns();
    let mut result = String::with_capacity(content.len());
    let mut in_block_comment = false;
    let mut in_docstring = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Handle block comments
        if let (Some(start), Some(end)) = (patterns.block_start, patterns.block_end) {
            if !in_docstring
                && trimmed.contains(start)
                && !trimmed.starts_with(patterns.doc_block_start.unwrap_or("###"))
            {
                in_block_comment = true;
            }
            if in_block_comment {
                if trimmed.contains(end) {
                    in_block_comment = false;
                }
                continue;
            }
        }

        // Handle Python docstrings (keep them in minimal mode)
        if *lang == Language::Python && trimmed.starts_with("\"\"\"") {
            in_docstring = !in_docstring;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        if in_docstring {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Skip single-line comments (but keep doc comments)
        if let Some(line_comment) = patterns.line
            && trimmed.starts_with(line_comment)
        {
            // Keep doc comments
            if let Some(doc) = patterns.doc_line
                && trimmed.starts_with(doc)
            {
                result.push_str(line);
                result.push('\n');
            }
            continue;
        }

        // Skip empty lines at this point, we'll normalize later
        if trimmed.is_empty() {
            result.push('\n');
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    // Normalize multiple blank lines to max 2
    let result = MULTIPLE_BLANK_LINES.replace_all(&result, "\n\n");
    result.trim().to_string()
}

/// Strip comments, collapse function bodies, keep signatures/imports/constants.
#[allow(dead_code)] // Aggressive mode temporarily disabled
pub(crate) fn filter_aggressive(content: &str, lang: &Language) -> String {
    let minimal = filter_minimal(content, lang);

    if lang == &Language::Unknown {
        // For unknown languages, just return the minimal filter result
        return minimal;
    }

    let mut result = String::with_capacity(minimal.len() / 2);
    let mut brace_depth = 0;
    let mut in_impl_body = false;

    for line in minimal.lines() {
        let trimmed = line.trim();

        // Always keep imports
        if IMPORT_PATTERN.is_match(trimmed) {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Always keep function/struct/class signatures
        if FUNC_SIGNATURE.is_match(trimmed) {
            result.push_str(line);
            result.push('\n');
            in_impl_body = true;
            brace_depth = 0;
            continue;
        }

        // Track brace depth for implementation bodies
        let open_braces = trimmed.matches('{').count();
        let close_braces = trimmed.matches('}').count();

        if in_impl_body {
            brace_depth += open_braces as i32;
            brace_depth -= close_braces as i32;

            // Only keep the opening and closing braces
            if brace_depth <= 1 && (trimmed == "{" || trimmed == "}" || trimmed.ends_with('{')) {
                result.push_str(line);
                result.push('\n');
            }

            if brace_depth <= 0 {
                in_impl_body = false;
                if !trimmed.is_empty() && trimmed != "}" {
                    result.push_str("    // ... implementation\n");
                }
            }
            continue;
        }

        // Keep type definitions, constants, etc.
        if trimmed.starts_with("const ")
            || trimmed.starts_with("static ")
            || trimmed.starts_with("let ")
            || trimmed.starts_with("pub const ")
            || trimmed.starts_with("pub static ")
        {
            result.push_str(line);
            result.push('\n');
        }
    }

    result.trim().to_string()
}

// --- Compressor ---

pub struct ReadCompressor;

impl ToolCompressor for ReadCompressor {
    fn compress(&self, arguments: &str, output: &str) -> Option<String> {
        let (fmt, numbered) = parse_numbered_lines(output);

        if numbered.len() < SMALL_THRESHOLD {
            return None;
        }

        let file_path = extract_file_path(arguments);

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
        let threshold = output.len() * 9 / 10;
        if compressed.len() >= threshold {
            return None;
        }

        Some(compressed)
    }
}

/// Which separator character the input uses for line numbers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LineFormat {
    /// Standard `cat -n`: `"     1\tcontent"`
    Tab,
    /// Claude Code Read tool: `"     1→content"`
    Arrow,
    /// OpenCode Read tool: `"1:content"`
    Colon,
}

/// Parse numbered-line output into (line_number, content) pairs,
/// also returning the detected format so the caller can round-trip faithfully.
/// Supports tab (`     1\t`), arrow (`1→`), and colon (`1:`) formats.
pub(crate) fn parse_numbered_lines(output: &str) -> (LineFormat, Vec<(Option<usize>, String)>) {
    let mut format = LineFormat::Tab; // default; overridden on first numbered line
    let mut format_detected = false;

    let lines = output
        .lines()
        .map(|line| {
            // Arrow format: "1→content"
            if let Some(pos) = line.find('→') {
                let prefix = line[..pos].trim();
                if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                    if !format_detected {
                        format = LineFormat::Arrow;
                        format_detected = true;
                    }
                    let num = prefix.parse::<usize>().ok();
                    return (num, line[pos + '→'.len_utf8()..].to_string());
                }
            }
            // Colon format: "1:content"
            if let Some(pos) = line.find(':') {
                let prefix = line[..pos].trim();
                if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                    if !format_detected {
                        format = LineFormat::Colon;
                        format_detected = true;
                    }
                    let num = prefix.parse::<usize>().ok();
                    return (num, line[pos + 1..].to_string());
                }
            }
            // Tab format: "     1\tcontent"
            if let Some(pos) = line.find('\t') {
                let prefix = line[..pos].trim();
                if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                    if !format_detected {
                        format = LineFormat::Tab;
                        format_detected = true;
                    }
                    let num = prefix.parse::<usize>().ok();
                    return (num, line[pos + 1..].to_string());
                }
            }
            (None, line.to_string())
        })
        .collect();

    (format, lines)
}

/// Format (line_number, content) pairs back using the same style as the original input.
pub(crate) fn format_numbered_lines(
    lines: &[(Option<usize>, String)],
    format: LineFormat,
) -> String {
    let mut parts = Vec::with_capacity(lines.len());
    for (num, content) in lines {
        if let Some(n) = num {
            let s = match format {
                LineFormat::Tab => format!("{n:>6}\t{content}"),
                LineFormat::Arrow => format!("{n:>6}→{content}"),
                LineFormat::Colon => format!("{n}:{content}"),
            };
            parts.push(s);
        } else {
            parts.push(content.clone());
        }
    }
    parts.join("\n")
}

/// Filter numbered lines, stripping comments while preserving original line numbers.
pub(crate) fn filter_minimal_numbered(
    lines: &[(Option<usize>, String)],
    lang: &Language,
) -> Vec<(Option<usize>, String)> {
    let patterns = lang.comment_patterns();
    let mut result: Vec<(Option<usize>, String)> = Vec::new();
    let mut in_block_comment = false;
    let mut in_docstring = false;
    let mut consecutive_blanks: usize = 0;

    for (num, line) in lines {
        let trimmed = line.trim();

        // Handle block comments
        if let (Some(start), Some(end)) = (patterns.block_start, patterns.block_end) {
            if !in_docstring
                && trimmed.contains(start)
                && !trimmed.starts_with(patterns.doc_block_start.unwrap_or("###"))
            {
                in_block_comment = true;
            }
            if in_block_comment {
                if trimmed.contains(end) {
                    in_block_comment = false;
                }
                continue;
            }
        }

        // Handle Python docstrings (keep them)
        if *lang == Language::Python && trimmed.starts_with("\"\"\"") {
            in_docstring = !in_docstring;
            consecutive_blanks = 0;
            result.push((*num, line.clone()));
            continue;
        }

        if in_docstring {
            consecutive_blanks = 0;
            result.push((*num, line.clone()));
            continue;
        }

        // Skip single-line comments (but keep doc comments)
        if let Some(line_comment) = patterns.line
            && trimmed.starts_with(line_comment)
        {
            if let Some(doc) = patterns.doc_line
                && trimmed.starts_with(doc)
            {
                consecutive_blanks = 0;
                result.push((*num, line.clone()));
            }
            continue;
        }

        // Blank lines: collapse to at most 2 consecutive
        if trimmed.is_empty() {
            if consecutive_blanks < 2 {
                result.push((*num, line.clone()));
                consecutive_blanks += 1;
            }
            continue;
        }

        consecutive_blanks = 0;
        result.push((*num, line.clone()));
    }

    // Trim leading/trailing blank lines
    while result.first().is_some_and(|(_, l)| l.trim().is_empty()) {
        result.remove(0);
    }
    while result.last().is_some_and(|(_, l)| l.trim().is_empty()) {
        result.pop();
    }

    result
}

/// Extract the file_path from Read tool arguments JSON.
fn extract_file_path(arguments: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| v.get("file_path")?.as_str().map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rust_file(n: usize) -> String {
        let mut lines = Vec::new();
        let mut ln = 1;
        lines.push(format!("     {}\tuse std::io;", ln));
        ln += 1;
        lines.push(format!("     {}\t", ln));
        ln += 1;
        lines.push(format!("     {}\t// This is a comment", ln));
        ln += 1;
        lines.push(format!("     {}\t/// Doc comment", ln));
        ln += 1;
        lines.push(format!("     {}\tfn main() {{", ln));
        ln += 1;
        for _ in 0..n {
            // Alternate: code line, then comment line (~50% comments)
            lines.push(format!("     {}\t    println!(\"hello\");", ln));
            ln += 1;
            lines.push(format!("     {}\t    // TODO: refactor this", ln));
            ln += 1;
        }
        lines.push(format!("     {}\t}}", ln));
        lines.join("\n")
    }

    fn make_args(path: &str) -> String {
        format!(r#"{{"file_path": "{}"}}"#, path)
    }

    #[test]
    fn test_small_output_not_compressed() {
        let output = make_rust_file(10);
        let compressor = ReadCompressor;
        assert!(
            compressor
                .compress(&make_args("/src/main.rs"), &output)
                .is_none()
        );
    }

    #[test]
    fn test_strips_comments_rust() {
        let output = make_rust_file(60);
        let args = make_args("/src/main.rs");
        let compressor = ReadCompressor;
        let result = compressor.compress(&args, &output);
        assert!(result.is_some());
        let compressed = result.unwrap();
        // Single-line comment should be stripped
        assert!(!compressed.contains("// This is a comment"));
        // Doc comment should be preserved
        assert!(compressed.contains("/// Doc comment"));
        // Import should be preserved
        assert!(compressed.contains("use std::io;"));
        // Function signature should be preserved
        assert!(compressed.contains("fn main()"));
    }

    #[test]
    fn test_large_file_uses_minimal_filter() {
        // 350 iterations * 2 lines each = 700+ lines — aggressive mode is disabled,
        // so this should still use minimal filtering only.
        let output = make_rust_file(350);
        let args = make_args("/src/main.rs");
        let compressor = ReadCompressor;
        let result = compressor.compress(&args, &output);
        assert!(result.is_some());
        let compressed = result.unwrap();
        // Should keep signature
        assert!(compressed.contains("fn main()"));
        // Should keep import
        assert!(compressed.contains("use std::io;"));
        // No system-reminder since aggressive mode is disabled
        assert!(!compressed.contains("<system-reminder>"));
    }

    #[test]
    fn test_compressed_output_preserves_line_numbers() {
        let output = make_rust_file(60);
        let compressor = ReadCompressor;
        let compressed = compressor.compress(&make_args("/src/main.rs"), &output).unwrap();
        // Line 3 (comment) is stripped; line 4 (doc comment) should keep number 4.
        assert!(
            compressed.contains("4\t/// Doc comment"),
            "doc comment should keep line number 4"
        );
        assert!(
            compressed.contains("1\tuse std::io;"),
            "import should keep line number 1"
        );
        // Stripped comment must not appear
        assert!(!compressed.contains("// This is a comment"));
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
    fn test_extract_file_path() {
        let args = r#"{"file_path": "/home/user/src/main.rs"}"#;
        assert_eq!(
            extract_file_path(args),
            Some("/home/user/src/main.rs".to_string())
        );
    }

    #[test]
    fn test_extract_file_path_missing() {
        assert_eq!(extract_file_path("{}"), None);
    }

    #[test]
    fn test_filter_minimal_strips_block_comments() {
        let code = "/* block comment */\nfn foo() {}\n";
        let result = filter_minimal(code, &Language::Rust);
        assert!(!result.contains("block comment"));
        assert!(result.contains("fn foo()"));
    }

    #[test]
    fn test_filter_minimal_collapses_blanks() {
        let code = "fn a() {}\n\n\n\n\nfn b() {}\n";
        let result = filter_minimal(code, &Language::Rust);
        assert!(result.contains("fn a()"));
        assert!(result.contains("fn b()"));
        // Should not have more than 2 consecutive newlines
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn test_filter_minimal_python_keeps_docstrings() {
        let code = "def foo():\n    \"\"\"Docstring.\"\"\"\n    pass\n";
        let result = filter_minimal(code, &Language::Python);
        assert!(result.contains("\"\"\"Docstring.\"\"\""));
    }

    #[test]
    fn test_filter_aggressive_keeps_signatures() {
        let code = "use std::io;\n\nfn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n";
        let result = filter_aggressive(code, &Language::Rust);
        assert!(result.contains("use std::io;"));
        assert!(result.contains("fn main()"));
        assert!(!result.contains("let x = 1"));
    }

    #[test]
    fn test_language_from_extension() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("tsx"), Language::TypeScript);
        assert_eq!(Language::from_extension("go"), Language::Go);
        assert_eq!(Language::from_extension("csv"), Language::Unknown);
    }

    #[test]
    fn test_empty_output() {
        let compressor = ReadCompressor;
        assert!(
            compressor
                .compress(&make_args("/src/main.rs"), "")
                .is_none()
        );
    }

    #[test]
    fn test_javascript_comments_stripped() {
        let mut lines = Vec::new();
        let mut ln = 1;
        lines.push(format!("     {}\timport React from 'react';", ln));
        ln += 1;
        lines.push(format!("     {}\t", ln));
        ln += 1;
        lines.push(format!("     {}\t// TODO: remove this later", ln));
        ln += 1;
        lines.push(format!("     {}\tfunction App() {{", ln));
        ln += 1;
        for _ in 0..30 {
            lines.push(format!("     {}\t  return <div>hello</div>;", ln));
            ln += 1;
            lines.push(format!("     {}\t  // comment line", ln));
            ln += 1;
        }
        lines.push(format!("     {}\t}}", ln));
        let output = lines.join("\n");
        let args = make_args("/src/App.jsx");
        let compressor = ReadCompressor;
        let result = compressor.compress(&args, &output);
        assert!(result.is_some());
        let compressed = result.unwrap();
        assert!(!compressed.contains("// TODO: remove this later"));
        assert!(!compressed.contains("// comment line"));
        assert!(compressed.contains("import React from 'react';"));
        assert!(compressed.contains("function App()"));
    }
}
