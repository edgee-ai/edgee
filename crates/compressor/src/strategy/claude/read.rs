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

use super::ToolCompressor;

/// Below this many content lines, don't compress at all.
const SMALL_THRESHOLD: usize = 50;

/// Above this many content lines, also collapse function bodies (brace
/// languages only). Comment-stripping alone leaves huge files barely smaller;
/// at this point the LLM gets more value from a dense skeleton than from full
/// bodies that consume context for marginal information.
const AGGRESSIVE_THRESHOLD: usize = 500;

/// Function bodies shorter than this are kept verbatim — collapsing tiny
/// bodies hurts readability without saving meaningful tokens.
const MIN_BODY_FOR_COLLAPSE: usize = 8;

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
    Toml,
    Json,
    Yaml,
    Markdown,
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
            "toml" => Language::Toml,
            "json" | "json5" | "jsonc" => Language::Json,
            "yaml" | "yml" => Language::Yaml,
            "md" | "markdown" => Language::Markdown,
            _ => Language::Unknown,
        }
    }

    /// Returns `true` for languages whose bodies are delimited by `{ }` —
    /// i.e. where the aggressive function-body collapse is meaningful.
    fn uses_braces(&self) -> bool {
        matches!(
            self,
            Language::Rust
                | Language::JavaScript
                | Language::TypeScript
                | Language::Go
                | Language::C
                | Language::Cpp
                | Language::Java
        )
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
            // Toml/Yaml use `#` like Shell. No block comments.
            Language::Toml | Language::Yaml => CommentPatterns {
                line: Some("#"),
                block_start: None,
                block_end: None,
                doc_line: None,
                doc_block_start: None,
            },
            // JSON has no comment syntax. JSON5/JSONC do, but treating them as
            // pure JSON is the safe default — we never strip valid content.
            Language::Json => CommentPatterns {
                line: None,
                block_start: None,
                block_end: None,
                doc_line: None,
                doc_block_start: None,
            },
            // Markdown has no real comment syntax (HTML comments exist but are
            // rare); treat it like JSON to avoid stripping content.
            Language::Markdown => CommentPatterns {
                line: None,
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

// --- Compressor ---

pub struct ReadCompressor;

impl ToolCompressor for ReadCompressor {
    fn compress(&self, arguments: &str, output: &str) -> Option<String> {
        let (fmt, numbered) = parse_numbered_lines(output);

        if numbered.len() < SMALL_THRESHOLD {
            return None;
        }

        let file_path = extract_file_path(arguments);

        // Lockfiles (Cargo.lock, package-lock.json, ...) are machine-generated
        // dumps where individual lines almost never matter to the LLM. Replace
        // the body with a stub keeping the first/last few lines so the agent
        // can still tell what file it's looking at.
        if let Some(path) = file_path.as_deref()
            && is_lockfile(path)
        {
            return Some(stub_lockfile(path, &numbered, fmt));
        }

        let lang = file_path
            .as_deref()
            .and_then(|p| Path::new(p).extension())
            .and_then(|e| e.to_str())
            .map(Language::from_extension)
            .unwrap_or(Language::Unknown);

        let mut filtered = filter_minimal_numbered(&numbered, &lang);

        if filtered.is_empty() {
            return None;
        }

        // For very large files in brace languages, also collapse function
        // bodies — comment-stripping alone leaves a 5000-line file at maybe
        // 4500 lines, which is still enormous.
        if filtered.len() > AGGRESSIVE_THRESHOLD && lang.uses_braces() {
            filtered = aggressive_collapse_braces(&filtered);
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

/// Returns `true` for the well-known dependency lock files. Match is on the
/// basename only — paths like `crates/foo/Cargo.lock` still trigger.
fn is_lockfile(path: &str) -> bool {
    let basename = path.rsplit('/').next().unwrap_or(path);
    matches!(
        basename,
        "Cargo.lock"
            | "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "Pipfile.lock"
            | "poetry.lock"
            | "uv.lock"
            | "Gemfile.lock"
            | "composer.lock"
            | "go.sum"
            | "bun.lockb"
            | "mix.lock"
    )
}

/// Replace a lockfile body with a short stub. Keeps the first 5 and last 3
/// lines of the original (numbered) so the LLM can still confirm what file
/// it is looking at; everything between is summarized as
/// `... <N> lockfile lines elided ...`.
pub(crate) fn stub_lockfile(
    path: &str,
    numbered: &[(Option<usize>, String)],
    fmt: LineFormat,
) -> String {
    const HEAD: usize = 5;
    const TAIL: usize = 3;

    if numbered.len() <= HEAD + TAIL {
        // Tiny lockfile — no meaningful elision possible.
        return format_numbered_lines(numbered, fmt);
    }

    let head: Vec<_> = numbered.iter().take(HEAD).cloned().collect();
    let tail_start = numbered.len() - TAIL;
    let tail: Vec<_> = numbered.iter().skip(tail_start).cloned().collect();
    let elided = numbered.len() - HEAD - TAIL;

    let mut out = format_numbered_lines(&head, fmt);
    out.push('\n');
    out.push_str(&format!(
        "... {elided} lockfile lines elided ({}) ...\n",
        path.rsplit('/').next().unwrap_or(path)
    ));
    out.push_str(&format_numbered_lines(&tail, fmt));
    out
}

/// Returns `true` if `trimmed` looks like the start of a callable / type
/// declaration whose body lives between `{` and the matching `}`.
///
/// Heuristic only — false positives just leave bodies expanded, false
/// negatives leave them collapsed but with too few lines to actually
/// trigger the body collapse threshold.
fn is_brace_body_signature(trimmed: &str) -> bool {
    // Rust
    if trimmed.starts_with("fn ")
        || trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub(")
        || trimmed.starts_with("async fn ")
        || trimmed.starts_with("pub async fn ")
        || trimmed.starts_with("impl ")
        || trimmed.starts_with("trait ")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("mod ")
    {
        return true;
    }
    // Go / C / C++ / Java / TS — common prefixes
    if trimmed.starts_with("func ")
        || trimmed.starts_with("function ")
        || trimmed.starts_with("export function ")
        || trimmed.starts_with("export default function ")
        || trimmed.starts_with("class ")
        || trimmed.starts_with("interface ")
        || trimmed.starts_with("public ")
        || trimmed.starts_with("private ")
        || trimmed.starts_with("protected ")
        || trimmed.starts_with("static ")
    {
        return true;
    }
    // Arrow function assignments: `const foo = (...) => {`
    if (trimmed.starts_with("const ") || trimmed.starts_with("let ") || trimmed.starts_with("var "))
        && trimmed.contains("=>")
    {
        return true;
    }
    false
}

/// Collapse function/class bodies in brace languages.
///
/// Walks the line list once, and whenever a "signature" line ending in `{`
/// opens a body that spans more than [`MIN_BODY_FOR_COLLAPSE`] lines, replaces
/// the body with a single `... (N lines)` placeholder. The signature line and
/// the matching closing brace are kept verbatim.
///
/// Brace counting is naive — it does not strip braces inside strings or
/// comments. Mis-counts only ever cause "kept too much" (the body fails to
/// collapse), never silent data loss.
pub(crate) fn aggressive_collapse_braces(
    lines: &[(Option<usize>, String)],
) -> Vec<(Option<usize>, String)> {
    let mut result: Vec<(Option<usize>, String)> = Vec::with_capacity(lines.len());
    let mut i = 0;

    while i < lines.len() {
        let (num, line) = &lines[i];
        let trimmed = line.trim();

        if trimmed.ends_with('{') && is_brace_body_signature(trimmed) {
            // Find the matching closing brace via depth counting.
            let mut depth: i32 = 0;
            let mut j = i;
            let mut closed = false;
            while j < lines.len() {
                for ch in lines[j].1.chars() {
                    match ch {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                if depth <= 0 {
                    closed = true;
                    break;
                }
                j += 1;
            }

            if closed && j > i {
                let body_len = j.saturating_sub(i + 1);
                if body_len >= MIN_BODY_FOR_COLLAPSE {
                    // Push signature, placeholder, closing line.
                    result.push((*num, line.clone()));
                    result.push((None, format!("    // ... ({body_len} lines collapsed)")));
                    result.push(lines[j].clone());
                    i = j + 1;
                    continue;
                }
            }
            // Body too small (or unbalanced braces) — keep verbatim.
        }

        result.push((*num, line.clone()));
        i += 1;
    }

    result
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
        let compressed = compressor
            .compress(&make_args("/src/main.rs"), &output)
            .unwrap();
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

    // --- Language extension tests ---

    #[test]
    fn test_language_from_extension_new_langs() {
        assert_eq!(Language::from_extension("toml"), Language::Toml);
        assert_eq!(Language::from_extension("json"), Language::Json);
        assert_eq!(Language::from_extension("json5"), Language::Json);
        assert_eq!(Language::from_extension("yaml"), Language::Yaml);
        assert_eq!(Language::from_extension("yml"), Language::Yaml);
        assert_eq!(Language::from_extension("md"), Language::Markdown);
        assert_eq!(Language::from_extension("markdown"), Language::Markdown);
    }

    #[test]
    fn test_json_no_comments_stripped() {
        // Direct unit test of the filter: JSON has no line-comment pattern,
        // so values that *look* like comments must survive verbatim.
        let lines = vec![
            (Some(1), "{".to_string()),
            (Some(2), "  \"url\": \"https://x.dev/foo\",".to_string()),
            (
                Some(3),
                "  \"note\": \"// this is a value, not a comment\",".to_string(),
            ),
            (Some(4), "  \"path\": \"/* fake block */\"".to_string()),
            (Some(5), "}".to_string()),
        ];
        let filtered = filter_minimal_numbered(&lines, &Language::Json);
        assert_eq!(
            filtered.len(),
            5,
            "JSON has no comment patterns; every line must be kept"
        );
        assert!(filtered[2].1.contains("// this is a value"));
        assert!(filtered[3].1.contains("/* fake block */"));
    }

    #[test]
    fn test_yaml_strips_hash_comments() {
        let mut lines = Vec::new();
        let mut ln = 1;
        lines.push(format!("     {}\tname: my-app", ln));
        ln += 1;
        lines.push(format!("     {}\t# Top-level comment", ln));
        ln += 1;
        for _ in 0..30 {
            lines.push(format!("     {}\t  field: value", ln));
            ln += 1;
            lines.push(format!("     {}\t  # field comment", ln));
            ln += 1;
        }
        let output = lines.join("\n");
        let args = make_args("/k8s/deploy.yaml");
        let compressor = ReadCompressor;
        let result = compressor.compress(&args, &output).unwrap();
        assert!(!result.contains("# Top-level comment"));
        assert!(!result.contains("# field comment"));
        assert!(result.contains("name: my-app"));
        assert!(result.contains("field: value"));
    }

    // --- Lockfile stub tests ---

    #[test]
    fn test_is_lockfile_recognizes_known_files() {
        assert!(is_lockfile("Cargo.lock"));
        assert!(is_lockfile("crates/foo/Cargo.lock"));
        assert!(is_lockfile("/abs/path/package-lock.json"));
        assert!(is_lockfile("yarn.lock"));
        assert!(is_lockfile("pnpm-lock.yaml"));
        assert!(is_lockfile("go.sum"));
        assert!(!is_lockfile("Cargo.toml"));
        assert!(!is_lockfile("src/main.rs"));
    }

    #[test]
    fn test_lockfile_replaced_with_stub() {
        // Build a fake Cargo.lock long enough to trigger compression.
        let mut lines = Vec::new();
        for i in 1..=200 {
            lines.push(format!("     {}\tname = \"crate-{}\"", i, i));
        }
        let output = lines.join("\n");
        let args = make_args("/proj/Cargo.lock");
        let compressor = ReadCompressor;
        let result = compressor.compress(&args, &output).unwrap();
        // Stub mentions elision and the file basename.
        assert!(result.contains("lockfile lines elided"));
        assert!(result.contains("Cargo.lock"));
        // Head and tail content preserved.
        assert!(result.contains("crate-1"));
        assert!(result.contains("crate-200"));
        // Heavily compressed: original is 200+ lines, stub should be tiny.
        assert!(result.lines().count() < 15);
    }

    // --- Aggressive mode (brace-body collapse) tests ---

    #[test]
    fn test_aggressive_collapse_for_large_rust_file() {
        // Build a 600-line Rust file with many small functions plus one fat
        // function whose body should be collapsed.
        let mut lines = Vec::new();
        let mut ln = 1usize;
        // 5 small functions (3 lines each, below MIN_BODY_FOR_COLLAPSE).
        for fn_i in 0..5 {
            lines.push(format!("     {}\tfn small_{}() {{", ln, fn_i));
            ln += 1;
            lines.push(format!("     {}\t    println!(\"hi\");", ln));
            ln += 1;
            lines.push(format!("     {}\t}}", ln));
            ln += 1;
        }
        // One fat function with a body well above the threshold.
        lines.push(format!("     {}\tfn fat_function() {{", ln));
        ln += 1;
        for _ in 0..600 {
            lines.push(format!("     {}\t    let _ = 1;", ln));
            ln += 1;
        }
        lines.push(format!("     {}\t}}", ln));

        let output = lines.join("\n");
        let args = make_args("/src/big.rs");
        let compressor = ReadCompressor;
        let result = compressor.compress(&args, &output).unwrap();

        // Fat function's signature and closing brace are kept; body collapses.
        assert!(result.contains("fn fat_function()"));
        assert!(result.contains("lines collapsed"));
        // Small functions stay verbatim (body too short for collapse).
        assert!(result.contains("fn small_0()"));
        // Compression must beat the 10% threshold to be returned at all,
        // so the total result is meaningfully shorter than the input.
        assert!(result.len() < output.len() * 9 / 10);
    }

    #[test]
    fn test_aggressive_mode_skipped_for_python() {
        // Python uses indentation, not braces — `uses_braces()` must be false.
        assert!(!Language::Python.uses_braces());
        assert!(!Language::Yaml.uses_braces());
        assert!(!Language::Json.uses_braces());
        assert!(!Language::Markdown.uses_braces());
        assert!(!Language::Toml.uses_braces());
        assert!(!Language::Ruby.uses_braces());
        assert!(!Language::Shell.uses_braces());
        // Brace languages stay enabled.
        assert!(Language::Rust.uses_braces());
        assert!(Language::JavaScript.uses_braces());
        assert!(Language::TypeScript.uses_braces());
        assert!(Language::Go.uses_braces());
        assert!(Language::C.uses_braces());
        assert!(Language::Cpp.uses_braces());
        assert!(Language::Java.uses_braces());
    }

    #[test]
    fn test_aggressive_collapse_keeps_small_bodies() {
        // Body of 3 lines (< MIN_BODY_FOR_COLLAPSE) — must stay verbatim.
        let lines = vec![
            (Some(1), "fn small() {".to_string()),
            (Some(2), "    let x = 1;".to_string()),
            (Some(3), "}".to_string()),
        ];
        let collapsed = aggressive_collapse_braces(&lines);
        assert_eq!(collapsed.len(), 3, "small bodies must not collapse");
        assert!(collapsed[1].1.contains("let x = 1"));
    }

    #[test]
    fn test_aggressive_collapse_collapses_large_bodies() {
        let mut lines = vec![(Some(1), "fn fat() {".to_string())];
        for i in 2..=20 {
            lines.push((Some(i), "    let _ = 1;".to_string()));
        }
        lines.push((Some(21), "}".to_string()));

        let collapsed = aggressive_collapse_braces(&lines);
        // Signature + placeholder + closing line.
        assert_eq!(collapsed.len(), 3);
        assert!(collapsed[0].1.contains("fn fat()"));
        assert!(collapsed[1].1.contains("lines collapsed"));
        assert_eq!(collapsed[2].1, "}");
    }
}
