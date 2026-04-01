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

//! Compressor for the Claude Code `Grep` tool output.
//!
//! The Grep tool has three output modes (extractable from the arguments JSON):
//! - `files_with_matches` (default): file paths, one per line → group by directory
//! - `content`: `path:line_num:content` lines → group by file, limit matches
//! - `count`: `path:N` lines → leave as-is (already compact)

use std::collections::HashMap;
use std::path::Path;

use super::ClaudeToolCompressor;

const MAX_LINE_LEN: usize = 120;
const MAX_MATCHES_PER_FILE: usize = 10;
const MAX_CONTEXT_PER_MATCH: usize = 5;
const MAX_TOTAL: usize = 50;
const MAX_PATH_LEN: usize = 50;

pub struct GrepCompressor;

impl ClaudeToolCompressor for GrepCompressor {
    fn compress(&self, arguments: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            tracing::debug!("grep: not compressing - output is empty");
            return None;
        }

        let mode = extract_output_mode(arguments);
        let pattern = extract_pattern(arguments);
        let single_file = extract_single_file_target(arguments);
        let context_lines = extract_context_lines(arguments);

        tracing::debug!(
            "grep: attempting compression with mode={:?}, pattern={:?}, single_file={:?}, context_lines={}",
            mode,
            pattern,
            single_file,
            context_lines
        );

        let result = match mode {
            OutputMode::FilesWithMatches => compress_files_with_matches(output),
            OutputMode::Content => compress_content(
                output,
                pattern.as_deref(),
                single_file.as_deref(),
                context_lines,
            ),
            OutputMode::Count => {
                tracing::debug!("grep: not compressing - count mode already compact");
                None
            }
        };

        if result.is_some() {
            tracing::debug!("grep: compression successful");
        } else {
            tracing::debug!("grep: compression returned None");
        }

        result
    }
}

#[derive(Debug, PartialEq)]
enum OutputMode {
    FilesWithMatches,
    Content,
    Count,
}

fn extract_output_mode(arguments: &str) -> OutputMode {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return OutputMode::FilesWithMatches;
    };
    match v.get("output_mode").and_then(|v| v.as_str()) {
        Some("content") => OutputMode::Content,
        Some("count") => OutputMode::Count,
        _ => OutputMode::FilesWithMatches,
    }
}

fn extract_pattern(arguments: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| v.get("pattern")?.as_str().map(String::from))
}

fn extract_single_file_target(arguments: &str) -> Option<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return None;
    };

    // Check if there's a "path" field that points to a single file
    if let Some(path_str) = v.get("path")?.as_str() {
        // Heuristic: if the path doesn't end with "/" and doesn't contain wildcards,
        // and the output has no filenames (starts with line numbers), treat as single file
        if !path_str.ends_with('/') && !path_str.contains('*') {
            // Extract just the filename part for prepending
            if let Some(filename) = path_str.split('/').next_back() {
                return Some(filename.to_string());
            }
        }
    }
    None
}

/// Extract the context line count from the arguments JSON.
/// Checks `context`, `-C`, `-A`, `-B` fields.
fn extract_context_lines(arguments: &str) -> usize {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return 0;
    };

    let mut max_ctx: usize = 0;

    // Check "context" and "-C" (synonyms)
    for key in &["context", "-C"] {
        if let Some(n) = v.get(*key).and_then(|v| v.as_u64()) {
            max_ctx = max_ctx.max(n as usize);
        }
    }

    // Check "-A" (after context) and "-B" (before context)
    for key in &["-A", "-B"] {
        if let Some(n) = v.get(*key).and_then(|v| v.as_u64()) {
            max_ctx = max_ctx.max(n as usize);
        }
    }

    max_ctx
}

/// Compress `files_with_matches` output: group paths by directory.
fn compress_files_with_matches(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let line_count = lines.len();

    if line_count < 20 {
        return None;
    }

    let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut by_ext: HashMap<String, usize> = HashMap::new();

    for line in &lines {
        let p = Path::new(line);
        let dir = p.parent().map(|d| d.to_str().unwrap_or(".")).unwrap_or(".");
        let dir = if dir.is_empty() { "." } else { dir };
        let filename = p
            .file_name()
            .map(|f| f.to_str().unwrap_or(""))
            .unwrap_or("");

        by_dir.entry(dir).or_default().push(filename);

        let ext = p
            .extension()
            .map(|e| format!(".{}", e.to_str().unwrap_or("")))
            .unwrap_or_else(|| "no ext".to_string());
        *by_ext.entry(ext).or_default() += 1;
    }

    let mut dirs: Vec<_> = by_dir.keys().copied().collect();
    dirs.sort();

    let total = lines.len();
    let mut out = format!("{}F {}D:\n\n", total, dirs.len());

    let mut shown = 0;
    let max_results = MAX_TOTAL;

    for dir in &dirs {
        if shown >= max_results {
            break;
        }

        let files_in_dir = &by_dir[dir];
        let remaining = max_results - shown;

        if files_in_dir.len() <= remaining {
            out.push_str(&format!("{}/ {}\n", dir, files_in_dir.join(" ")));
            shown += files_in_dir.len();
        } else {
            let partial: Vec<&str> = files_in_dir.iter().take(remaining).copied().collect();
            out.push_str(&format!("{}/ {}\n", dir, partial.join(" ")));
            break;
        }
    }

    // Truncation indicator removed — directory grouping already shows relevant results

    if by_ext.len() > 1 {
        let mut exts: Vec<_> = by_ext.iter().collect();
        exts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let ext_parts: Vec<String> = exts
            .iter()
            .take(5)
            .map(|(e, c)| format!("{}({})", e, c))
            .collect();
        out.push_str(&format!("\next: {}\n", ext_parts.join(" ")));
    }

    Some(out)
}

/// A parsed grep output line (match or context).
struct GrepOutputLine<'a> {
    file: &'a str,
    line_num: usize,
    content: &'a str,
    #[allow(dead_code)]
    is_match: bool,
}

/// Normalize file path: convert absolute paths to relative if possible.
fn normalize_path(path: &str) -> &str {
    // If it starts with /home/clement/work/, strip that prefix
    if let Some(stripped) = path.strip_prefix("/home/clement/work/") {
        return stripped;
    }
    path
}

/// Extract the file path from a grep line and normalize it.
/// A grep line looks like "path:linenum:content" or "path-linenum-content"
/// We need to extract just the "path" part and normalize it.
/// The key insight: the separator comes after the filename, so for context lines
/// we look for the pattern "digits-" which indicates "linenum-content"
fn extract_and_normalize_prefix(line: &str) -> Option<String> {
    // Strategy: look for the pattern that marks the separator
    // For match lines: path:linenum: (contains `:`)
    // For context lines: path-linenum- (the linenum is all digits before the `-`)

    // First check for match line pattern: path:digits:
    let parts: Vec<&str> = line.splitn(2, ':').collect();
    if parts.len() == 2 && parts[0].contains('/') {
        // Could be a match line
        if parts[1]
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            // Likely a match line: path:linenum...
            let normalized = normalize_path(parts[0]);
            return Some(normalized.to_string());
        }
    }

    // Check for context line pattern: path-linenum-
    // We need to find where "path" ends and "linenum" begins
    // The path ends with "/" and linenum is all digits followed by "-"
    if let Some(last_slash) = line.rfind('/') {
        let after_slash = &line[last_slash + 1..];
        // Look for "digits-" pattern
        if let Some(dash_pos) = after_slash.find('-') {
            let maybe_num = &after_slash[..dash_pos];
            if maybe_num.chars().all(|c| c.is_ascii_digit()) {
                // This looks like linenum- pattern
                let path_part = &line[..last_slash + 1 + dash_pos];
                // Remove the trailing "-linenum" part if present
                if let Some(path_end) = path_part.rfind('-') {
                    let path_only = &path_part[..path_end];
                    let normalized = normalize_path(path_only);
                    return Some(normalized.to_string());
                }
            }
        }
    }

    None
}

/// Normalize all paths in the output to convert absolute paths to relative
fn normalize_all_output_paths(output: &str) -> String {
    output
        .lines()
        .map(|line| {
            // Find the first : or - (separator between path and rest)
            for (i, ch) in line.char_indices() {
                if (ch == ':' || ch == '-') && i > 0 {
                    let path_part = &line[..i];
                    let normalized_path = normalize_path(path_part);
                    if normalized_path != path_part {
                        // Path was absolute, normalize it
                        let sep = ch;
                        let rest = &line[i + 1..];
                        return format!("{}{}{}", normalized_path, sep, rest);
                    }
                    break;
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Detect if output lacks filenames (single-file mode without explicit path prefix).
/// If the first non-empty line starts with a number followed by `:` or `-` (without a path prefix),
/// prepend the filename to each line.
fn prepend_filename_if_needed_tool(output: &str, filename: &str) -> String {
    let first_line = output.lines().find(|l| !l.trim().is_empty());

    if let Some(line) = first_line {
        // Check if it starts with a number (indicating no filename prefix)
        // Pattern: digits followed by : or - (like "123:content" or "123-content")
        if line
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            let looks_like_linenums = line
                .split(':')
                .next()
                .map(|s| s.parse::<usize>().is_ok())
                .unwrap_or(false)
                || line
                    .split('-')
                    .next()
                    .map(|s| s.parse::<usize>().is_ok())
                    .unwrap_or(false);

            if looks_like_linenums {
                return output
                    .lines()
                    .map(|l| {
                        if l.is_empty() {
                            l.to_string()
                        } else if l.starts_with("--") {
                            // Keep block separators as-is
                            l.to_string()
                        } else {
                            // Prepend filename
                            format!("{}:{}", filename, l)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
        }
    }

    output.to_string()
}

/// Parse a match line: `file:linenum:content`
/// Match lines MUST have the format: path:digits:content
/// If the line contains `-digits-` pattern before `:digits:`, it's a context line, not a match line
fn parse_match_line_content(line: &str) -> Option<GrepOutputLine<'_>> {
    // Reject if line contains context line pattern (path-digits-...) before match line pattern
    // Look for "-digits-" which indicates a context line
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '-' {
            // Check if next characters are digits
            let mut digit_count = 0;
            let saved_pos = chars.clone();
            while let Some(&peek_ch) = chars.peek() {
                if peek_ch.is_ascii_digit() {
                    digit_count += 1;
                    chars.next();
                } else {
                    break;
                }
            }
            if digit_count > 0 && chars.peek() == Some(&'-') {
                // Found "-digits-" pattern, this is a context line
                return None;
            }
            // Reset if we didn't find the pattern
            chars = saved_pos;
        }
    }

    // Now try to parse as match line
    let parts: Vec<&str> = line.splitn(3, ':').collect();
    if parts.len() == 3 {
        let file = normalize_path(parts[0]);
        if let Ok(ln) = parts[1].trim().parse::<usize>() {
            return Some(GrepOutputLine {
                file,
                line_num: ln,
                content: parts[2],
                is_match: true,
            });
        }
        // Fallback: parts[1] is not a number, treat as file:content
        return Some(GrepOutputLine {
            file,
            line_num: 0,
            content: &line[parts[0].len() + 1..],
            is_match: true,
        });
    }
    if parts.len() == 2 {
        let file = normalize_path(parts[0]);
        return Some(GrepOutputLine {
            file,
            line_num: 0,
            content: parts[1],
            is_match: true,
        });
    }
    None
}

/// Parse a context line: `file-linenum-content` or `file-content` using known files.
fn parse_context_line_content<'a>(
    line: &'a str,
    known_files: &std::collections::HashSet<&'a str>,
) -> Option<GrepOutputLine<'a>> {
    // First check if the line's path (when normalized) matches any known file
    if let Some(normalized_prefix) = extract_and_normalize_prefix(line) {
        for file in known_files {
            if *file == normalized_prefix {
                // This is a match! Now parse the rest of the line
                // We need to find where the file prefix ends and the rest begins
                for (i, ch) in line.char_indices() {
                    if ch == '-' && i > 0 {
                        let path_part = &line[..i];
                        if normalize_path(path_part) == *file {
                            let rest = &line[i + 1..];
                            // Try "linenum-content"
                            if let Some(dash_pos) = rest.find('-') {
                                let maybe_num = &rest[..dash_pos];
                                if let Ok(ln) = maybe_num.parse::<usize>()
                                    && ln > 0
                                {
                                    return Some(GrepOutputLine {
                                        file,
                                        line_num: ln,
                                        content: &rest[dash_pos + 1..],
                                        is_match: false,
                                    });
                                }
                            }
                            // No line number — just "content"
                            return Some(GrepOutputLine {
                                file,
                                line_num: 0,
                                content: rest,
                                is_match: false,
                            });
                        }
                    }
                }
            }
        }
    }

    // Fallback: try matching with relative paths directly
    for file in known_files {
        let dash_prefix = format!("{}-", file);
        if let Some(rest) = line.strip_prefix(&dash_prefix) {
            // Try "linenum-content"
            if let Some(dash_pos) = rest.find('-') {
                let maybe_num = &rest[..dash_pos];
                if let Ok(ln) = maybe_num.parse::<usize>()
                    && ln > 0
                {
                    return Some(GrepOutputLine {
                        file,
                        line_num: ln,
                        content: &rest[dash_pos + 1..],
                        is_match: false,
                    });
                }
            }
            // No line number — just "content"
            return Some(GrepOutputLine {
                file,
                line_num: 0,
                content: rest,
                is_match: false,
            });
        }
    }
    None
}

/// Split output into `--`-delimited blocks.
fn split_blocks_content(raw: &str) -> Vec<Vec<&str>> {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in raw.lines() {
        if line == "--" {
            if !current.is_empty() {
                blocks.push(current);
                current = Vec::new();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// Select which lines to display for a file, prioritizing match lines over context.
/// Always includes all match lines (up to `max`), fills remaining budget with context.
fn select_lines(matches: &[(usize, String, bool)], max: usize) -> Vec<(usize, String, bool)> {
    if matches.len() <= max {
        return matches.to_vec();
    }

    let match_lines: Vec<_> = matches
        .iter()
        .filter(|(_, _, is_match)| *is_match)
        .collect();

    // If match lines alone exceed budget, just take first `max` match lines.
    if match_lines.len() >= max {
        return match_lines.into_iter().take(max).cloned().collect();
    }

    // Budget for context lines around matches.
    let context_budget = max - match_lines.len();

    // Build a set of indices we want to keep: all match indices + nearby context.
    let match_indices: Vec<usize> = matches
        .iter()
        .enumerate()
        .filter(|(_, (_, _, is_match))| *is_match)
        .map(|(i, _)| i)
        .collect();

    let mut keep = vec![false; matches.len()];
    for &idx in &match_indices {
        keep[idx] = true;
    }

    // Distribute context budget around matches, trying to center them.
    let mut remaining = context_budget;
    let per_match = if match_indices.is_empty() {
        context_budget
    } else {
        (context_budget / match_indices.len()).max(1)
    };

    for &idx in &match_indices {
        if remaining == 0 {
            break;
        }
        let mut budget = per_match.min(remaining);

        let mut distance = 1;
        while budget > 0 && (idx >= distance || idx + distance < matches.len()) {
            // Try after first
            if idx + distance < matches.len() && budget > 0 {
                let after_idx = idx + distance;
                if !keep[after_idx] && !matches[after_idx].2 {
                    keep[after_idx] = true;
                    budget -= 1;
                    remaining -= 1;
                }
            }
            // Try before
            if idx >= distance && budget > 0 {
                let before_idx = idx - distance;
                if !keep[before_idx] && !matches[before_idx].2 {
                    keep[before_idx] = true;
                    budget -= 1;
                    remaining -= 1;
                }
            }
            distance += 1;
        }
    }

    // If there's still budget, fill with remaining context lines in order.
    if remaining > 0 {
        let context_indices: Vec<usize> = matches
            .iter()
            .enumerate()
            .filter(|(_, (_, _, is_match))| !*is_match)
            .map(|(i, _)| i)
            .collect();
        for idx in context_indices {
            if remaining == 0 {
                break;
            }
            if !keep[idx] {
                keep[idx] = true;
                remaining -= 1;
            }
        }
    }

    matches
        .iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, entry)| entry.clone())
        .collect()
}

/// Compress `content` output: `path:line_num:content` grouped by file, with context support.
fn compress_content(
    output: &str,
    pattern: Option<&str>,
    single_file: Option<&str>,
    context_lines: usize,
) -> Option<String> {
    // If this is single-file mode and output has no filename prefix (just linenum:content),
    // prepend the filename to each line
    let processed_output = if let Some(filename) = single_file {
        prepend_filename_if_needed_tool(output, filename)
    } else {
        output.to_string()
    };

    // Normalize all paths in the output to handle mixed absolute/relative paths
    let normalized_output = normalize_all_output_paths(&processed_output);

    let blocks = split_blocks_content(&normalized_output);

    let mut by_file: HashMap<&str, Vec<(usize, String, bool)>> = HashMap::new();
    let mut total = 0;

    // Process each block: find filename from match lines, then parse context lines
    for block in blocks.iter() {
        // Find the file for this block by trying parse_match_line on all lines
        let mut block_file: Option<&str> = None;
        let mut best_count = 0;

        for line in block {
            if let Some(parsed) = parse_match_line_content(line)
                && !parsed.file.is_empty()
            {
                let colon_prefix = format!("{}:", parsed.file);
                let dash_prefix = format!("{}-", parsed.file);

                let count = block
                    .iter()
                    .filter(|l| {
                        // Direct match: relative path
                        if l.starts_with(&colon_prefix) || l.starts_with(&dash_prefix) {
                            return true;
                        }
                        // Check if line has absolute path that normalizes to this file
                        if let Some(normalized) = extract_and_normalize_prefix(l) {
                            return normalized == parsed.file;
                        }
                        false
                    })
                    .count();
                if count > best_count {
                    best_count = count;
                    block_file = Some(parsed.file);
                }
            }
        }

        let mut block_known: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if let Some(f) = block_file {
            block_known.insert(f);
        }

        // Parse all lines in this block
        for line in block {
            // Try context line first (more specific)
            if let Some(parsed) = parse_context_line_content(line, &block_known) {
                total += 1;
                let cleaned = clean_line(parsed.content, MAX_LINE_LEN, pattern);
                by_file
                    .entry(parsed.file)
                    .or_default()
                    .push((parsed.line_num, cleaned, false));
                continue;
            }

            // Then try match line
            if let Some(parsed) = parse_match_line_content(line) {
                total += 1;
                let cleaned = clean_line(parsed.content, MAX_LINE_LEN, pattern);
                by_file
                    .entry(parsed.file)
                    .or_default()
                    .push((parsed.line_num, cleaned, true));
                continue;
            }
        }
    }

    if total == 0 {
        return None;
    }

    if total < 10 {
        return None;
    }

    let mut out = format!("{} in {}F:\n\n", total, by_file.len());

    let mut shown = 0;
    let mut files: Vec<_> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| *f);

    for (file, matches) in files {
        if shown >= MAX_TOTAL {
            break;
        }

        let file_display = compact_path(file);
        out.push_str(&format!("{} ({}):\n", file_display, matches.len()));

        // Use context-aware line selection like the bash grep compressor:
        // when context was requested, honour that many context lines per match.
        let num_matches = matches.iter().filter(|(_, _, m)| *m).count().max(1);
        let budget = if context_lines > 0 {
            let ctx = context_lines.min(MAX_CONTEXT_PER_MATCH);
            num_matches * (ctx * 2 + 1)
        } else {
            MAX_MATCHES_PER_FILE
        };
        let selected = select_lines(matches, budget);
        for (line_num, content, is_match) in &selected {
            if *line_num > 0 {
                let sep = if *is_match { ':' } else { '-' };
                out.push_str(&format!("  {:>4}{} {}\n", line_num, sep, content));
            } else {
                out.push_str(&format!("  {}\n", content));
            }
            shown += 1;
            if shown >= MAX_TOTAL {
                break;
            }
        }

        out.push('\n');
    }

    Some(out)
}

/// Clean and truncate a line, centering on the pattern match if present.
fn clean_line(line: &str, max_len: usize, pattern: Option<&str>) -> String {
    let trimmed = line.trim();

    if trimmed.len() <= max_len {
        return trimmed.to_string();
    }

    // If we have a pattern, try to center the truncation on it
    if let Some(pat) = pattern {
        let lower = trimmed.to_lowercase();
        let pattern_lower = pat.to_lowercase();

        if let Some(pos) = lower.find(&pattern_lower) {
            let start = trimmed.floor_char_boundary(pos.saturating_sub(max_len / 3));
            let end = trimmed.ceil_char_boundary((start + max_len).min(trimmed.len()));
            let start = if end == trimmed.len() {
                trimmed.floor_char_boundary(end.saturating_sub(max_len))
            } else {
                start
            };

            let slice = &trimmed[start..end];
            return if start > 0 && end < trimmed.len() {
                format!("...{}...", slice)
            } else if start > 0 {
                format!("...{}", slice)
            } else {
                format!("{}...", slice)
            };
        }
    }

    // Fallback: simple prefix truncation
    format!(
        "{}...",
        &trimmed[..trimmed.floor_char_boundary(max_len.saturating_sub(3))]
    )
}

/// Compact a long path by eliding middle directories.
fn compact_path(path: &str) -> String {
    if path.len() <= MAX_PATH_LEN {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }

    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_output_mode_default() {
        assert_eq!(extract_output_mode("{}"), OutputMode::FilesWithMatches);
    }

    #[test]
    fn test_extract_output_mode_content() {
        assert_eq!(
            extract_output_mode(r#"{"output_mode": "content"}"#),
            OutputMode::Content
        );
    }

    #[test]
    fn test_extract_output_mode_count() {
        assert_eq!(
            extract_output_mode(r#"{"output_mode": "count"}"#),
            OutputMode::Count
        );
    }

    #[test]
    fn test_extract_output_mode_invalid_json() {
        assert_eq!(
            extract_output_mode("not json"),
            OutputMode::FilesWithMatches
        );
    }

    #[test]
    fn test_files_with_matches_small_not_compressed() {
        let output = "src/main.rs\nsrc/lib.rs\n";
        let compressor = GrepCompressor;
        assert!(compressor.compress("{}", output).is_none());
    }

    #[test]
    fn test_files_with_matches_large_compressed() {
        let paths: Vec<String> = (0..30)
            .map(|i| format!("src/components/file{}.ts", i))
            .collect();
        let output = paths.join("\n");
        let compressor = GrepCompressor;
        let result = compressor.compress("{}", &output);
        assert!(result.is_some());
        let compressed = result.unwrap();
        assert!(compressed.contains("30F 1D:"));
        assert!(compressed.contains("src/components/"));
    }

    #[test]
    fn test_content_mode_compressed() {
        let mut lines = Vec::new();
        for i in 1..=20 {
            lines.push(format!("src/main.rs:{}:fn function_{}() {{}}", i * 10, i));
        }
        let output = lines.join("\n");
        let args = r#"{"output_mode": "content"}"#;
        let compressor = GrepCompressor;
        let result = compressor.compress(args, &output);
        assert!(result.is_some());
        let compressed = result.unwrap();
        assert!(compressed.contains("20 in 1F:"));
        assert!(compressed.contains("src/main.rs (20):"));
    }

    #[test]
    fn test_content_mode_small_not_compressed() {
        let output = "src/main.rs:1:fn main() {}\nsrc/main.rs:2:}\n";
        let args = r#"{"output_mode": "content"}"#;
        let compressor = GrepCompressor;
        assert!(compressor.compress(args, output).is_none());
    }

    #[test]
    fn test_count_mode_not_compressed() {
        let output = "src/main.rs:5\nsrc/lib.rs:3\n";
        let args = r#"{"output_mode": "count"}"#;
        let compressor = GrepCompressor;
        assert!(compressor.compress(args, output).is_none());
    }

    #[test]
    fn test_empty_output() {
        let compressor = GrepCompressor;
        assert!(compressor.compress("{}", "").is_none());
        assert!(compressor.compress("{}", "  \n  \n").is_none());
    }

    #[test]
    fn test_content_truncates_long_lines() {
        let long_content = "x".repeat(200);
        let mut lines = Vec::new();
        for i in 1..=15 {
            lines.push(format!("src/main.rs:{}:{}", i, long_content));
        }
        let output = lines.join("\n");
        let args = r#"{"output_mode": "content"}"#;
        let compressor = GrepCompressor;
        let result = compressor.compress(args, &output).unwrap();
        for line in result.lines() {
            if line.starts_with("  ") && line.contains(": ") {
                assert!(
                    line.len() <= MAX_LINE_LEN + 20,
                    "line too long: {}",
                    line.len()
                );
            }
        }
    }

    #[test]
    fn test_content_limits_matches_per_file() {
        let mut lines = Vec::new();
        for i in 1..=25 {
            lines.push(format!("src/main.rs:{}:line {}", i, i));
        }
        let output = lines.join("\n");
        let args = r#"{"output_mode": "content"}"#;
        let compressor = GrepCompressor;
        let result = compressor.compress(args, &output).unwrap();
        assert!(result.contains("src/main.rs (25):"));
        // Shows first 10 matches, truncates remaining 15 (no +15 indicator anymore)
        assert!(result.contains("line 1"));
        assert!(result.contains("line 10"));
    }

    #[test]
    fn test_files_with_matches_extension_summary() {
        let mut paths = Vec::new();
        for i in 0..15 {
            paths.push(format!("src/file{}.rs", i));
        }
        for i in 0..10 {
            paths.push(format!("src/file{}.ts", i));
        }
        let output = paths.join("\n");
        let compressor = GrepCompressor;
        let result = compressor.compress("{}", &output).unwrap();
        assert!(result.contains("ext:"));
        assert!(result.contains(".rs(15)"));
        assert!(result.contains(".ts(10)"));
    }

    #[test]
    fn test_clean_line_short() {
        let line = "  const result = someFunction();  ";
        let cleaned = clean_line(line, 50, Some("result"));
        assert_eq!(cleaned, "const result = someFunction();");
    }

    #[test]
    fn test_clean_line_centers_on_pattern() {
        let line = "x".repeat(50) + "PATTERN" + &"y".repeat(50);
        let cleaned = clean_line(&line, 50, Some("pattern"));
        assert!(cleaned.contains("PATTERN"));
        assert!(cleaned.starts_with("...") || cleaned.ends_with("..."));
    }

    #[test]
    fn test_clean_line_no_pattern_truncates_prefix() {
        let line = "x".repeat(200);
        let cleaned = clean_line(&line, 50, None);
        assert!(cleaned.ends_with("..."));
        assert!(cleaned.len() <= 53); // 50 + "..."
    }

    #[test]
    fn test_compact_path_short() {
        let path = "src/main.rs";
        assert_eq!(compact_path(path), "src/main.rs");
    }

    #[test]
    fn test_compact_path_long() {
        let path = "/Users/patrick/dev/project/src/components/Button.tsx";
        let compact = compact_path(path);
        assert!(compact.contains("..."));
        assert!(compact.contains("components"));
        assert!(compact.contains("Button.tsx"));
    }

    #[test]
    fn test_extract_pattern() {
        let args = r#"{"pattern": "TODO", "output_mode": "content"}"#;
        assert_eq!(extract_pattern(args), Some("TODO".to_string()));
    }

    #[test]
    fn test_extract_pattern_missing() {
        assert_eq!(extract_pattern("{}"), None);
    }

    #[test]
    fn test_content_with_context_lines() {
        // Simulate grep -A output with context lines using - separator
        let input = "\
edgee-cli/openapi/openapi.json-2424-    \"/v1/users/me\": {
edgee-cli/openapi/openapi.json-2425-      \"get\": {
edgee-cli/openapi/openapi.json:2426:        \"operationId\": \"getMe\",
edgee-cli/openapi/openapi.json-2427-        \"summary\": \"Get my User object\",
edgee-cli/openapi/openapi.json-2428-        \"description\": \"Retrieves my current User object.\",
--
edgee-cli/openapi/openapi.json-2449-          }
edgee-cli/openapi/openapi.json-2450-        }
edgee-cli/openapi/openapi.json:2451:        \"operationId\": \"updateMe\",
edgee-cli/openapi/openapi.json-2452-        \"summary\": \"Update my User\",
edgee-cli/openapi/openapi.json-2453-        \"description\": \"Updates the current user\",
--
src/main.rs-1-fn main() {
src/main.rs:2:    // operationId: helper
src/main.rs-3-    println!(\"hello\");
--
src/lib.rs-10-pub fn init() {
src/lib.rs:11:    // operationId: start
src/lib.rs-12-}
";
        let args = r#"{"output_mode": "content"}"#;
        let compressor = GrepCompressor;
        let result = compressor.compress(args, input).unwrap();
        // Match lines show with ':' separator
        assert!(result.contains("2426: \"operationId\": \"getMe\","));
        // Context lines show with '-' separator
        assert!(result.contains("2427- \"summary\": \"Get my User object\","));
        assert!(result.contains("edgee-cli/openapi/openapi.json"));
    }

    #[test]
    fn test_content_with_absolute_paths() {
        // Grep tool might return absolute paths - should be normalized
        let input = "\
/home/clement/work/edgee-cli/openapi/openapi.json-2424-    \"/v1/users/me\": {
/home/clement/work/edgee-cli/openapi/openapi.json-2425-      \"get\": {
/home/clement/work/edgee-cli/openapi/openapi.json:2426:        \"operationId\": \"getMe\",
/home/clement/work/edgee-cli/openapi/openapi.json-2427-        \"summary\": \"Get my User object\",
/home/clement/work/edgee-cli/openapi/openapi.json-2428-        \"description\": \"Retrieves my current User object.\",
/home/clement/work/edgee-cli/openapi/openapi.json-2429-        \"responses\": {},
--
edgee-cli/openapi/openapi.json:2451:        \"operationId\": \"updateMe\",
edgee-cli/openapi/openapi.json-2452-        \"summary\": \"Update my User\",
edgee-cli/openapi/openapi.json-2453-        \"description\": \"Updates the current user\",
edgee-cli/openapi/openapi.json-2454-        \"parameters\": [],
edgee-cli/openapi/openapi.json-2455-        \"responses\": {},
";
        let args = r#"{"output_mode": "content"}"#;
        let compressor = GrepCompressor;
        let result = compressor.compress(args, input).unwrap();
        // Should normalize paths
        assert!(result.contains("edgee-cli/openapi/openapi.json"));
        // Should not have absolute paths in output
        assert!(!result.contains("/home/clement/work/"));
    }

    #[test]
    fn test_single_file_grep_with_line_numbers() {
        // Single file grep outputs: linenum:content (no filename prefix)
        // When path is provided in args, should prepend filename
        let input = "\
10:fn main() {
11:    let x = 1;
20:fn other() {
21:    let y = 2;
30:fn third() {
31:    let z = 3;
40:fn fourth() {
41:    let w = 4;
50:fn fifth() {
51:    let v = 5;
60:fn sixth() {
61:    let u = 6;
";
        let args = r#"{"output_mode": "content", "path": "main.rs"}"#;
        let compressor = GrepCompressor;
        let result = compressor.compress(args, input).unwrap();
        assert!(result.contains("main.rs"));
        assert!(result.contains("12 in 1F:"));
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn test_single_file_with_context_lines() {
        // Single file grep -A output: linenum:content, linenum-context, --
        let input = "\
9-// before
10:fn main() {
11-    let x = 1;
--
19-// before2
20:fn other() {
21-    let y = 2;
--
29-// before3
30:fn third() {
31-    let z = 3;
--
39-// before4
40:fn fourth() {
41-    let w = 4;
";
        let args = r#"{"output_mode": "content", "path": "src/main.rs"}"#;
        let compressor = GrepCompressor;
        let result = compressor.compress(args, input).unwrap();
        assert!(result.contains("main.rs"));
        assert!(result.contains("12 in 1F:"));
        assert!(result.contains("10: fn main()"));
        assert!(result.contains("11- ") && result.contains("let x"));
    }

    #[test]
    fn test_mixed_absolute_and_relative_paths() {
        // Grep tool may return mixed absolute and relative paths for the same file
        // Should be grouped together, not treated as separate files
        let input = "\
/home/clement/work/edgee-cli/openapi/openapi.json-2396-              \"type\": \"string\"
/home/clement/work/edgee-cli/openapi/openapi.json:2426:        \"operationId\": \"getMe\",
edgee-cli/openapi/openapi.json-2427-        \"summary\": \"Get my User\",
edgee-cli/openapi/openapi.json:2451:        \"operationId\": \"updateMe\",
/home/clement/work/edgee-cli/openapi/openapi.json-2452-        \"summary\": \"Update my User\",
edgee-cli/openapi/openapi.json-2453-        \"description\": \"Updates the user\",
/home/clement/work/edgee-cli/openapi/openapi.json:2460:        \"operationId\": \"deleteMe\",
edgee-cli/openapi/openapi.json-2461-        \"summary\": \"Delete my User\",
/home/clement/work/edgee-cli/openapi/openapi.json-2462-        \"description\": \"Deletes the user\",
edgee-cli/openapi/openapi.json:2470:        \"operationId\": \"updateProfile\",
";
        let args = r#"{"output_mode": "content"}"#;
        let compressor = GrepCompressor;
        let result = compressor.compress(args, input).unwrap();
        // Should have only 1 file, not 2
        assert!(result.contains("10 in 1F:"));
        assert!(result.contains("edgee-cli/openapi/openapi.json"));
        // Should NOT have created bogus files
        assert!(!result.contains("openapi2.json"));
        assert!(!result.contains("/home/clement/work/"));
    }
}
