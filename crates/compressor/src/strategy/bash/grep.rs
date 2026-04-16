//! Compressor for `grep` / `rg` command output.
//!
//! Groups matches by file, strips leading whitespace, and truncates
//! long lines to produce a compact search result listing.
//!
//! Handles context flags (-A, -B, -C) where grep uses `file-content` for
//! context lines and `file:content` for match lines. Filenames are discovered
//! from match lines (`:` is unambiguous) then used to parse context lines.

use std::collections::{BTreeMap, HashSet};

use super::BashCompressor;

const MAX_LINE_LEN: usize = 120;
const MAX_MATCHES_PER_FILE: usize = 10;
const MAX_CONTEXT_PER_MATCH: usize = 5;

pub struct GrepCompressor;

impl BashCompressor for GrepCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let info = parse_grep_command(command);
        compact_grep(output, &info)
    }
}

/// Information extracted from a grep command.
#[derive(Debug)]
struct GrepCommandInfo {
    /// Single-file target, or None for recursive/multi-file searches.
    single_file: Option<String>,
    /// When `-r` is used with a single positional target, store it here.
    /// We'll check at output-processing time whether grep actually prefixed
    /// filenames (directory target) or not (file target).
    recursive_single_target: Option<String>,
    /// Whether -n / --line-number was present.
    has_line_numbers: bool,
    /// Max context lines requested via -A/-B/-C (0 = no context flags).
    context_lines: usize,
}

/// Tokenize a shell command respecting single and double quotes.
///
/// Strips quote characters from the output; a backslash outside single quotes
/// escapes the next character.  This is a best-effort approximation of POSIX
/// shell word-splitting — enough to correctly count positional arguments even
/// when the grep pattern contains spaces (e.g. `'"foo bar"'`).
fn shell_tokenize(s: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Split a command line on unquoted `|` characters (pipeline segments).
///
/// Respects single quotes, double quotes, and backslash escapes so that a
/// `|` inside a quoted grep pattern (e.g. `grep "a\|b" file | head`) is not
/// treated as a pipe.
fn shell_split_pipes(s: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(c);
            }
            '\\' if !in_single => {
                current.push(c);
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '|' if !in_single && !in_double => {
                segments.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    segments.push(current);
    segments
}

/// Parse a grep command, which may be part of a pipeline.
///
/// Handles combined short flags (`-rnA30`), long flags (`--line-number`),
/// flags with inline values (`-A30`, `--after-context=30`), and
/// flags with separate values (`-A 30`).
fn parse_grep_command(command: &str) -> GrepCommandInfo {
    // Find the grep segment in a pipeline by splitting on `|`, respecting quotes.
    // A `|` inside single or double quotes (e.g. `grep "a\|b" file`) is NOT a pipe.
    let segments = shell_split_pipes(command);
    let grep_part = segments
        .iter()
        .find(|s| {
            let t = s.trim();
            t == "grep" || t.starts_with("grep ") || t.starts_with("grep\t")
        })
        .map(|s| s.as_str())
        .unwrap_or(command);

    let all_tokens = shell_tokenize(grep_part);

    // Skip the "grep" token itself.
    let tokens: &[String] = if all_tokens
        .first()
        .map(|s| s == "grep" || s.ends_with("/grep"))
        .unwrap_or(false)
    {
        &all_tokens[1..]
    } else {
        &all_tokens[..]
    };

    let mut has_line_numbers = false;
    let mut is_recursive = false;
    let mut after_context: usize = 0;
    let mut before_context: usize = 0;
    let mut positional: Vec<&str> = Vec::new();
    let mut after_dashdash = false;
    let mut i = 0;

    /// Parse a decimal integer from a byte slice, returning 0 on failure.
    fn parse_usize(s: &[u8]) -> usize {
        s.iter()
            .take_while(|b| b.is_ascii_digit())
            .fold(0usize, |acc, &b| acc * 10 + (b - b'0') as usize)
    }

    while i < tokens.len() {
        let tok: &str = &tokens[i];

        if after_dashdash {
            positional.push(tok);
            i += 1;
            continue;
        }

        if tok == "--" {
            after_dashdash = true;
            i += 1;
            continue;
        }

        if tok.starts_with("--") {
            // Long flag, possibly with an inline value: --after-context=30
            let (opt, inline_val) = match tok.find('=') {
                Some(eq) => (&tok[..eq], Some(&tok[eq + 1..])),
                None => (tok, None),
            };
            match opt {
                "--recursive" => is_recursive = true,
                "--line-number" => has_line_numbers = true,
                "--after-context" => {
                    let val = inline_val.unwrap_or_else(|| {
                        i += 1;
                        tokens.get(i).map(|s| s.as_str()).unwrap_or("0")
                    });
                    after_context = after_context.max(val.parse().unwrap_or(0));
                }
                "--before-context" => {
                    let val = inline_val.unwrap_or_else(|| {
                        i += 1;
                        tokens.get(i).map(|s| s.as_str()).unwrap_or("0")
                    });
                    before_context = before_context.max(val.parse().unwrap_or(0));
                }
                "--context" => {
                    let val = inline_val.unwrap_or_else(|| {
                        i += 1;
                        tokens.get(i).map(|s| s.as_str()).unwrap_or("0")
                    });
                    let n: usize = val.parse().unwrap_or(0);
                    after_context = after_context.max(n);
                    before_context = before_context.max(n);
                }
                // Flags that consume the next token as their value.
                "--max-count" | "--label" | "--include" | "--exclude" | "--exclude-dir"
                | "--color" | "--colour" => {
                    if inline_val.is_none() {
                        i += 1; // skip value token
                    }
                }
                _ => {}
            }
        } else if tok.starts_with('-') && tok.len() > 1 {
            // Short flag(s), possibly combined: -rnA30
            let bytes = &tok.as_bytes()[1..];
            let mut j = 0;
            while j < bytes.len() {
                match bytes[j] {
                    b'r' | b'R' => is_recursive = true,
                    b'n' => has_line_numbers = true,
                    b'A' => {
                        if j + 1 < bytes.len() {
                            after_context = after_context.max(parse_usize(&bytes[j + 1..]));
                            j = bytes.len() - 1;
                        } else if let Some(val) = tokens.get(i + 1) {
                            after_context = after_context.max(val.parse().unwrap_or(0));
                            i += 1;
                        }
                    }
                    b'B' => {
                        if j + 1 < bytes.len() {
                            before_context = before_context.max(parse_usize(&bytes[j + 1..]));
                            j = bytes.len() - 1;
                        } else if let Some(val) = tokens.get(i + 1) {
                            before_context = before_context.max(val.parse().unwrap_or(0));
                            i += 1;
                        }
                    }
                    b'C' => {
                        let n = if j + 1 < bytes.len() {
                            let v = parse_usize(&bytes[j + 1..]);
                            j = bytes.len() - 1;
                            v
                        } else if let Some(val) = tokens.get(i + 1) {
                            let v = val.parse().unwrap_or(0);
                            i += 1;
                            v
                        } else {
                            0
                        };
                        after_context = after_context.max(n);
                        before_context = before_context.max(n);
                    }
                    // Flags that consume a value (inline or next token).
                    b'e' | b'f' | b'm' | b'D' | b'd' => {
                        if j + 1 < bytes.len() {
                            j = bytes.len() - 1;
                        } else {
                            i += 1;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        } else {
            positional.push(tok);
        }

        i += 1;
    }

    // positional[0] = pattern, positional[1..] = file/dir targets.
    // Single-file mode: exactly one target and not recursive.
    let (single_file, recursive_single_target) = if positional.len() == 2 {
        if is_recursive {
            // `-r` with a single target: might be a file (no prefix in output)
            // or a directory (prefix in output). We'll check at output time.
            (None, Some(positional[1].to_string()))
        } else {
            (Some(positional[1].to_string()), None)
        }
    } else {
        (None, None)
    };

    GrepCommandInfo {
        single_file,
        recursive_single_target,
        has_line_numbers,
        context_lines: after_context.max(before_context),
    }
}

/// Prepend the filename to every output line when the grep output lacks a filename prefix.
///
/// For `grep -n` (line numbers): match lines start with `N:content`, context lines with `N-content`.
/// We prepend `filename:` or `filename-` accordingly so the rest of the parser sees the normal
/// `file:linenum:content` / `file-linenum-content` format.
///
/// For `grep` without `-n` (no line numbers): all lines are bare content, so we prepend `filename:`
/// to every line (treating them all as match lines).
fn prepend_filename_if_needed(output: &str, info: &GrepCommandInfo) -> String {
    let filename = match &info.single_file {
        Some(f) => f.as_str(),
        None => return output.to_string(),
    };

    output
        .lines()
        .map(|l| {
            if l.is_empty() || l.starts_with("--") {
                return l.to_string();
            }
            if info.has_line_numbers {
                // Determine separator from the line itself: digits then `:` (match) or `-` (context).
                let first_non_digit = l.bytes().position(|b| !b.is_ascii_digit());
                let starts_with_digit = l
                    .bytes()
                    .next()
                    .map(|b| b.is_ascii_digit())
                    .unwrap_or(false);
                if starts_with_digit && let Some(pos) = first_non_digit {
                    let sep = l.as_bytes()[pos];
                    if sep == b'-' {
                        return format!("{}-{}", filename, l);
                    }
                }
                format!("{}:{}", filename, l)
            } else {
                // Bare content: no line numbers, all lines are match lines.
                format!("{}:{}", filename, l)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A parsed grep output line.
struct GrepLine<'a> {
    file: &'a str,
    line_num: usize,
    content: &'a str,
    /// true for match lines (`:`), false for context lines (`-`)
    #[allow(dead_code)]
    is_match: bool,
}

/// Parse a match line (`:` separated). Returns (file, line_num, content).
/// Handles both `file:linenum:content` and `file:content` formats.
fn parse_match_line(line: &str) -> Option<GrepLine<'_>> {
    let parts: Vec<&str> = line.splitn(3, ':').collect();
    if parts.len() == 3 {
        if let Ok(ln) = parts[1].trim().parse::<usize>()
            && ln > 0
        {
            return Some(GrepLine {
                file: parts[0],
                line_num: ln,
                content: parts[2],
                is_match: true,
            });
        }
        // parts[1] wasn't a line number — treat as "file:content" with rest joined
        let content = &line[parts[0].len() + 1..];
        return Some(GrepLine {
            file: parts[0],
            line_num: 0,
            content,
            is_match: true,
        });
    }
    if parts.len() == 2 {
        return Some(GrepLine {
            file: parts[0],
            line_num: 0,
            content: parts[1],
            is_match: true,
        });
    }
    None
}

/// Parse a context line using known filenames.
/// Context lines use `-` as separator: `file-linenum-content` or `file-content`.
fn parse_context_line<'a>(line: &'a str, known_files: &HashSet<&'a str>) -> Option<GrepLine<'a>> {
    for file in known_files {
        let prefix = format!("{}-", file);
        if let Some(rest) = line.strip_prefix(&prefix) {
            // Try "linenum-content"
            if let Some(dash_pos) = rest.find('-') {
                let maybe_num = &rest[..dash_pos];
                if let Ok(ln) = maybe_num.parse::<usize>()
                    && ln > 0
                {
                    let content = &rest[dash_pos + 1..];
                    return Some(GrepLine {
                        file,
                        line_num: ln,
                        content,
                        is_match: false,
                    });
                }
            }
            // No line number — just "content"
            return Some(GrepLine {
                file,
                line_num: 0,
                content: rest,
                is_match: false,
            });
        }
    }
    None
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
    let context_lines: Vec<_> = matches
        .iter()
        .filter(|(_, _, is_match)| !*is_match)
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

    // For each match, include surrounding context (prefer lines just before/after).
    let mut keep = vec![false; matches.len()];
    for &idx in &match_indices {
        keep[idx] = true;
    }

    // Distribute context budget around matches, trying to center them.
    // For each match, expand outward equally before and after to keep match centered.
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

        // Expand symmetrically around the match (after first so odd budget lines
        // land after the match, keeping it closer to the center of its context window).
        let mut distance = 1;
        while budget > 0 && (idx >= distance || idx + distance < matches.len()) {
            // Try after first (so with an odd remaining budget, the extra line
            // goes after the match rather than before it).
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
        for (i, _) in context_lines.iter().enumerate() {
            if remaining == 0 {
                break;
            }
            let orig_idx = matches
                .iter()
                .enumerate()
                .filter(|(_, (_, _, is_match))| !*is_match)
                .nth(i)
                .map(|(idx, _)| idx)
                .unwrap();
            if !keep[orig_idx] {
                keep[orig_idx] = true;
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

/// Split raw grep output into `--`-delimited blocks.
/// Lines not separated by `--` form a single block.
fn split_blocks(raw: &str) -> Vec<Vec<&str>> {
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

fn compact_grep(raw: &str, info: &GrepCommandInfo) -> Option<String> {
    // When `-r` was used with a single target, check whether the output lines
    // actually start with that target path.  If they don't, grep treated the
    // target as a file (not a directory) and we should use single-file mode.
    let promoted;
    let info = if info.single_file.is_none() {
        if let Some(ref target) = info.recursive_single_target {
            let has_prefix = raw.lines().any(|l| l != "--" && l.starts_with(target));
            if !has_prefix {
                promoted = GrepCommandInfo {
                    single_file: Some(target.clone()),
                    recursive_single_target: None,
                    has_line_numbers: info.has_line_numbers,
                    context_lines: info.context_lines,
                };
                &promoted
            } else {
                info
            }
        } else {
            info
        }
    } else {
        info
    };

    // In single-file mode the output has no filename prefix; prepend it so
    // the rest of the parser sees the standard `file:linenum:content` format.
    let processed_output = if info.single_file.is_some() {
        prepend_filename_if_needed(raw, info)
    } else {
        raw.to_string()
    };

    let blocks = split_blocks(&processed_output);

    // Process each block: find the filename from match lines (`:` separator),
    // then use it to parse context lines (`-` separator) in the same block.
    let mut by_file: BTreeMap<&str, Vec<(usize, String, bool)>> = BTreeMap::new();
    let mut total = 0;

    for block in blocks.iter() {
        // Find the filename for this block from match lines.
        // We pick the candidate file that appears as a prefix (`file:` or `file-`)
        // on the most lines in the block. This avoids picking a bogus file from
        // context lines that happen to contain `:` in their content.
        let mut block_file: Option<&str> = None;
        let mut best_count = 0;
        for line in block {
            if let Some(parsed) = parse_match_line(line)
                && !parsed.file.is_empty()
            {
                let colon_prefix = format!("{}:", parsed.file);
                let dash_prefix = format!("{}-", parsed.file);
                let count = block
                    .iter()
                    .filter(|l| l.starts_with(&colon_prefix) || l.starts_with(&dash_prefix))
                    .count();
                if count > best_count {
                    best_count = count;
                    block_file = Some(parsed.file);
                }
            }
        }
        let mut block_known: HashSet<&str> = HashSet::new();
        if let Some(f) = block_file {
            block_known.insert(f);
        }

        // Now parse all lines in this block.
        for line in block {
            // Try context line first using the block's known file.
            if let Some(parsed) = parse_context_line(line, &block_known) {
                total += 1;
                let cleaned = truncate_line(parsed.content.trim(), MAX_LINE_LEN);
                by_file
                    .entry(parsed.file)
                    .or_default()
                    .push((parsed.line_num, cleaned, false));
                continue;
            }

            // Then try match line.
            if let Some(parsed) = parse_match_line(line) {
                total += 1;
                let cleaned = truncate_line(parsed.content.trim(), MAX_LINE_LEN);
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

    for (file, matches) in &by_file {
        let file_display = compact_path(file);
        out.push_str(&format!("{} ({}):\n", file_display, matches.len()));

        // Always show match lines plus surrounding context.
        // When the user requested context (-A/-B/-C), honour that many
        // context lines per match so we don't throw away what they asked for.
        let num_matches = matches.iter().filter(|(_, _, m)| *m).count().max(1);
        let budget = if info.context_lines > 0 {
            let ctx = info.context_lines.min(MAX_CONTEXT_PER_MATCH);
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
        }

        out.push('\n');
    }

    Some(out)
}

fn truncate_line(line: &str, max_len: usize) -> String {
    if line.len() <= max_len {
        line.to_string()
    } else {
        let end = max_len.saturating_sub(3);
        // Find a valid char boundary at or before `end`
        let end = line.floor_char_boundary(end);
        format!("{}...", &line[..end])
    }
}

fn compact_path(path: &str) -> &str {
    // Just return as-is for the compressor (no emoji, keep path readable)
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: calls compact_grep with no single-file mode and no context flags.
    fn compact_grep(input: &str) -> Option<String> {
        super::compact_grep(
            input,
            &GrepCommandInfo {
                single_file: None,
                recursive_single_target: None,
                has_line_numbers: false,
                context_lines: 0,
            },
        )
    }

    fn single_file_info(file: &str, has_line_numbers: bool) -> GrepCommandInfo {
        GrepCommandInfo {
            single_file: Some(file.to_string()),
            recursive_single_target: None,
            has_line_numbers,
            context_lines: 0,
        }
    }

    // ── shell_split_pipes ─────────────────────────────────────────────

    #[test]
    fn test_shell_split_pipes_simple() {
        let segs = shell_split_pipes("grep -rn foo | head -20");
        assert_eq!(segs.len(), 2);
        assert!(segs[0].contains("grep"));
        assert!(segs[1].contains("head"));
    }

    #[test]
    fn test_shell_split_pipes_quoted_pipe() {
        // The \| inside double quotes must NOT be treated as a pipe
        let segs = shell_split_pipes(r#"grep -A 30 "get_me\|/me" /some/file.json | head -50"#);
        assert_eq!(segs.len(), 2);
        assert!(
            segs[0].contains("/some/file.json"),
            "file path should stay in grep segment, got: {:?}",
            segs[0]
        );
    }

    #[test]
    fn test_shell_split_pipes_single_quoted_pipe() {
        let segs = shell_split_pipes("grep 'a|b' file.txt | wc -l");
        assert_eq!(segs.len(), 2);
        assert!(segs[0].contains("file.txt"));
    }

    // ── parse_grep_command ────────────────────────────────────────────

    #[test]
    fn test_parse_pattern_with_pipe_in_quotes() {
        // grep -A 30 "get_me\|/me" /some/file.json | head -50
        // The \| is inside double quotes — should NOT split the command
        let info = parse_grep_command(r#"grep -A 30 "get_me\|/me" /some/file.json | head -50"#);
        assert_eq!(
            info.single_file.as_deref(),
            Some("/some/file.json"),
            "should detect single file despite pipe in pattern"
        );
    }

    #[test]
    fn test_parse_quoted_pattern_with_spaces() {
        // grep -rnC 30 '"operationId": "getMe"' edgee-cli/openapi/openapi.json
        // The pattern contains spaces and is wrapped in single quotes.
        // split_whitespace() would incorrectly split it into multiple tokens,
        // making positional.len() > 2 and preventing single-file detection.
        let info = parse_grep_command(
            r#"grep -rnC 30 '"operationId": "getMe"' edgee-cli/openapi/openapi.json"#,
        );
        assert!(info.has_line_numbers, "should detect -n");
        // -r makes single_file None, but the target is stored in recursive_single_target
        // so we can check the output at processing time.
        assert!(info.single_file.is_none());
        assert_eq!(
            info.recursive_single_target.as_deref(),
            Some("edgee-cli/openapi/openapi.json"),
        );
    }

    #[test]
    fn test_parse_quoted_pattern_no_recursive() {
        // grep -nC 30 '"operationId": "getMe"' edgee-cli/openapi/openapi.json
        // Without -r, with exactly one file target, should detect single-file mode.
        let info = parse_grep_command(
            r#"grep -nC 30 '"operationId": "getMe"' edgee-cli/openapi/openapi.json"#,
        );
        assert!(info.has_line_numbers, "should detect -n");
        assert_eq!(
            info.single_file.as_deref(),
            Some("edgee-cli/openapi/openapi.json"),
            "should detect single file despite quoted pattern with spaces"
        );
    }

    // ── helpers ──────────────────────────────────────────────────────

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate_line("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let long = "a".repeat(200);
        let result = truncate_line(&long, 120);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 120);
    }

    // ── thresholds ──────────────────────────────────────────────────

    #[test]
    fn test_empty_input() {
        let compressor = GrepCompressor;
        assert!(compressor.compress("grep -rn 'xyz' .", "").is_none());
    }

    #[test]
    fn test_whitespace_only() {
        let compressor = GrepCompressor;
        assert!(
            compressor
                .compress("grep -rn 'xyz' .", "  \n  \n")
                .is_none()
        );
    }

    #[test]
    fn test_below_threshold_not_compressed() {
        // 3 matches is below the 10-line threshold
        let input = "src/main.rs:10:fn main() {\nsrc/main.rs:15:    println!(\"hello\");\nsrc/lib.rs:1:pub mod utils;\n";
        assert!(compact_grep(input).is_none());
    }

    #[test]
    fn test_limits_matches_per_file() {
        let mut input = String::new();
        for i in 1..=20 {
            input.push_str(&format!("src/main.rs:{}:line {}\n", i, i));
        }
        let result = compact_grep(&input).unwrap();
        assert!(result.contains("src/main.rs (20):"));
        // Shows first 10 matches, truncates remaining 10 (no +10 indicator anymore)
        assert!(result.contains("line 1"));
        assert!(result.contains("line 10"));
    }

    #[test]
    fn test_strips_leading_whitespace() {
        let mut lines = Vec::new();
        for i in 1..=15 {
            lines.push(format!("src/main.rs:{}:    fn main() {{", i * 10));
        }
        let input = lines.join("\n");
        let result = compact_grep(&input).unwrap();
        assert!(result.contains("fn main()"));
    }

    // ── 1. grep -rn (recursive + line numbers) ─────────────────────

    #[test]
    fn test_grep_rn() {
        // Format: file:linenum:content
        let mut lines = Vec::new();
        for i in 1..=15 {
            lines.push(format!("src/main.rs:{}:fn function_{}() {{}}", i, i));
        }
        let input = lines.join("\n");
        let result = compact_grep(&input).unwrap();
        assert!(result.contains("15 in 1F:"));
        assert!(result.contains("src/main.rs (15):"));
        assert!(result.contains("   1: fn function_1()"));
    }

    // ── 2. grep -r (recursive, no line numbers) ────────────────────

    #[test]
    fn test_grep_r() {
        // Format: file:content
        let mut lines = Vec::new();
        for i in 0..15 {
            lines.push(format!("src/file{}.rs:fn something() {{}}", i));
        }
        let input = lines.join("\n");
        let result = compact_grep(&input).unwrap();
        assert!(result.contains("15 in 15F:"));
        // No line numbers in output
        assert!(!result.contains("   0:"));
    }

    // ── 3. grep -rnA/B/C (recursive + line numbers + context) ──────

    #[test]
    fn test_grep_rn_context() {
        // Format: match file:linenum:content, context file-linenum-content, separator --
        let input = "\
src/main.rs-9-// before
src/main.rs:10:fn main() {
src/main.rs-11-    let x = 1;
--
src/main.rs-19-// before2
src/main.rs:20:fn other() {
src/main.rs-21-    let y = 2;
--
src/lib.rs-4-use std::io;
src/lib.rs:5:pub fn init() {
src/lib.rs-6-    println!(\"init\");
--
src/lib.rs-14-use std::fs;
src/lib.rs:15:pub fn load() {
src/lib.rs-16-    println!(\"load\");
--
src/util.rs-1-// header
src/util.rs:2:fn helper() {
src/util.rs-3-    todo!()
--
src/util.rs-9-// other
src/util.rs:10:fn helper2() {
src/util.rs-11-    todo!()
";
        let result = compact_grep(input).unwrap();
        assert!(result.contains("in 3F:"));
        // Match lines show linenum + ':'
        assert!(result.contains("  10: fn main()"));
        // Context lines show linenum + '-'
        assert!(result.contains("   9- // before"));
        assert!(result.contains("  11- let x = 1;"));
        // -- separators are stripped
        assert!(!result.contains("--"));
    }

    #[test]
    fn test_grep_rn_context_dashes_in_filename() {
        // Dashed filenames like edgee-cli/... with -rnA
        let input = "\
edgee-cli/openapi/openapi.json:2426:        \"operationId\": \"getMe\",
edgee-cli/openapi/openapi.json-2427-        \"summary\": \"Get my User object\",
edgee-cli/openapi/openapi.json-2428-        \"description\": \"Returns the current user\",
--
edgee-cli/openapi/openapi.json:2500:        \"operationId\": \"listOrgs\",
edgee-cli/openapi/openapi.json-2501-        \"summary\": \"List organizations\",
edgee-cli/openapi/openapi.json-2502-        \"description\": \"Returns all orgs\",
--
my-app/src/main.rs:10:fn hello() {
my-app/src/main.rs-11-    println!(\"hi\");
my-app/src/main.rs-12-}
--
my-app/src/main.rs:20:fn world() {
my-app/src/main.rs-21-    println!(\"world\");
";
        let result = compact_grep(input).unwrap();
        assert!(result.contains("in 2F:"));
        assert!(result.contains("edgee-cli/openapi/openapi.json"));
        assert!(result.contains("2427- \"summary\": \"Get my User object\","));
        assert!(result.contains("my-app/src/main.rs"));
        // No bogus filenames with mangled dash prefixes
        assert!(!result.contains("json-"));
    }

    // ── 4. grep -rA/B/C (recursive, no line numbers, context) ──────

    #[test]
    fn test_grep_r_context() {
        // Format: match file:content, context file-content, separator --
        let input = "\
src/main.rs:fn main() {
src/main.rs-    let x = 1;
src/main.rs-    let y = 2;
--
src/main.rs:fn other() {
src/main.rs-    let z = 3;
--
src/lib.rs:pub fn init() {
src/lib.rs-    println!(\"init\");
--
src/lib.rs:pub fn load() {
src/lib.rs-    println!(\"load\");
--
src/util.rs:fn helper() {
src/util.rs-    todo!()
";
        let result = compact_grep(input).unwrap();
        assert!(result.contains("in 3F:"));
        // No line numbers anywhere
        assert!(!result.contains("   0"));
        // Content preserved
        assert!(result.contains("fn main()"));
        assert!(result.contains("let x = 1;"));
    }

    #[test]
    fn test_grep_r_context_dashes_in_filename() {
        // The hardest case: dashed filenames + no line numbers + context with `:` in content
        let input = "\
edgee-cli/openapi/openapi.json:        \"operationId\": \"getMe\",
edgee-cli/openapi/openapi.json-        \"summary\": \"Get my User object\",
edgee-cli/openapi/openapi.json-        \"description\": \"Returns the current user\",
--
edgee-cli/openapi/openapi.json:        \"operationId\": \"listOrgs\",
edgee-cli/openapi/openapi.json-        \"summary\": \"List organizations\",
edgee-cli/openapi/openapi.json-        \"description\": \"Returns all orgs\",
--
my-app/src/my-module.rs:fn hello() {
my-app/src/my-module.rs-    println!(\"hi\");
my-app/src/my-module.rs-}
--
my-app/src/my-module.rs:fn world() {
my-app/src/my-module.rs-    println!(\"world\");
";
        let result = compact_grep(input).unwrap();
        assert!(result.contains("in 2F:"));
        assert!(result.contains("edgee-cli/openapi/openapi.json"));
        assert!(result.contains("my-app/src/my-module.rs"));
        assert!(result.contains("\"summary\": \"Get my User object\","));
        // No mangled filenames
        assert!(!result.contains("json-"));
    }

    // ── 5. grep -n (single file, line numbers) ─────────────────────

    #[test]
    fn test_grep_n_single_file() {
        // Format: linenum:content (no filename prefix)
        // Each "linenum:content" parses as a separate "file" since there's no real filename.
        // This is expected — single-file grep -n output is inherently ambiguous.
        let mut lines = Vec::new();
        for i in 1..=12 {
            lines.push(format!("{}:fn function_{}() {{}}", i, i));
        }
        let input = lines.join("\n");
        let result = compact_grep(&input).unwrap();
        assert!(result.contains("12 in 12F:"));
        assert!(result.contains("fn function_1()"));
    }

    // ── 6. grep (single file, bare — no flags) ─────────────────────

    #[test]
    fn test_grep_bare_single_file() {
        // Format: just content lines, no prefix at all
        // These lines have no `:` so they'll be unparseable — nothing to compress
        let mut lines = Vec::new();
        for i in 0..15 {
            lines.push(format!("fn function_{}() {{}}", i));
        }
        let input = lines.join("\n");
        // No `:` in any line, can't parse file or line number
        assert!(compact_grep(&input).is_none());
    }

    // ── 7. grep -nA/B/C (single file, line numbers + context) ──────

    #[test]
    fn test_grep_n_context_single_file() {
        // Format: match linenum:content, context linenum-content, separator --
        // Without a filename prefix, match lines look like "10:content"
        // and context lines look like "9-content" or "11-content".
        // Each match line parses as a separate "file" since line numbers look like filenames.
        // Context lines won't match known "files" (which are "10", "20", etc.) so they're lost.
        // This is inherently ambiguous — single-file grep -nA can't be reliably parsed.
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
        let result = compact_grep(input);
        // Only 4 match lines parse (context lines are lost) — below threshold
        assert!(result.is_none());
    }

    // ── 8. grep -A/B/C (single file, no line numbers, context) ─────

    #[test]
    fn test_grep_context_single_file_no_linenums() {
        // Format: just content lines with -- separators, no prefix at all
        // Lines with `:` in content may parse, lines without won't
        let input = "\
\"operationId\": \"getMe\",
\"summary\": \"Get my User object\",
\"description\": \"Returns the current user\",
--
\"operationId\": \"listOrgs\",
\"summary\": \"List organizations\",
\"description\": \"Returns all orgs\",
--
\"operationId\": \"createOrg\",
\"summary\": \"Create organization\",
\"description\": \"Creates a new org\",
--
\"operationId\": \"deleteOrg\",
\"summary\": \"Delete organization\",
";
        // Lines with `:` will parse as file:content with weird "files"
        // This is a best-effort case — single-file grep without -n or -r
        // produces output we can't reliably distinguish from random text
        let result = compact_grep(input);
        // May or may not compress depending on how many lines parse
        // The important thing is it doesn't panic
        if let Some(r) = result {
            assert!(!r.is_empty());
        }
    }

    // ── 9. grep -l (filenames only) ─────────────────────────────────

    #[test]
    fn test_grep_l_filenames_only() {
        // Format: one filename per line, no `:` or `-` separators
        let input = "\
src/main.rs
src/lib.rs
src/util.rs
src/config.rs
src/auth.rs
src/db.rs
src/api.rs
src/routes.rs
src/models.rs
src/helpers.rs
src/tests.rs
";
        // No `:` in any line — can't parse
        assert!(compact_grep(input).is_none());
    }

    // ── 10. grep -c / grep -rc (count mode) ────────────────────────

    #[test]
    fn test_grep_rc_count() {
        // Format: file:count
        let mut lines = Vec::new();
        for i in 0..15 {
            lines.push(format!("src/file{}.rs:42", i));
        }
        let input = lines.join("\n");
        // These parse as file:content with content="42"
        let result = compact_grep(&input).unwrap();
        assert!(result.contains("15 in 15F:"));
    }

    #[test]
    fn test_grep_c_single_file_count() {
        // Format: just a number
        let input = "42\n";
        assert!(compact_grep(input).is_none());
    }

    // ── unit tests for parse helpers ────────────────────────────────

    #[test]
    fn test_parse_match_line_with_linenum() {
        let parsed = parse_match_line("src/main.rs:10:fn main() {").unwrap();
        assert_eq!(parsed.file, "src/main.rs");
        assert_eq!(parsed.line_num, 10);
        assert_eq!(parsed.content, "fn main() {");
        assert!(parsed.is_match);
    }

    #[test]
    fn test_parse_match_line_without_linenum() {
        let parsed = parse_match_line("src/main.rs:fn main() {").unwrap();
        assert_eq!(parsed.file, "src/main.rs");
        assert_eq!(parsed.line_num, 0);
        assert_eq!(parsed.content, "fn main() {");
        assert!(parsed.is_match);
    }

    #[test]
    fn test_parse_match_line_content_with_colons() {
        // JSON content has colons — should keep full content after file:linenum:
        let parsed =
            parse_match_line("openapi.json:10:        \"operationId\": \"getMe\",").unwrap();
        assert_eq!(parsed.file, "openapi.json");
        assert_eq!(parsed.line_num, 10);
        assert_eq!(parsed.content, "        \"operationId\": \"getMe\",");
    }

    #[test]
    fn test_parse_context_with_known_file() {
        let mut known = HashSet::new();
        known.insert("edgee-cli/openapi/openapi.json");
        let parsed = parse_context_line(
            "edgee-cli/openapi/openapi.json-        \"summary\": \"Get a User\",",
            &known,
        )
        .unwrap();
        assert_eq!(parsed.file, "edgee-cli/openapi/openapi.json");
        assert_eq!(parsed.line_num, 0);
        assert_eq!(parsed.content, "        \"summary\": \"Get a User\",");
        assert!(!parsed.is_match);
    }

    #[test]
    fn test_parse_context_with_linenum_and_known_file() {
        let mut known = HashSet::new();
        known.insert("edgee-cli/openapi/openapi.json");
        let parsed = parse_context_line(
            "edgee-cli/openapi/openapi.json-2427-        \"summary\": \"Get a User\",",
            &known,
        )
        .unwrap();
        assert_eq!(parsed.file, "edgee-cli/openapi/openapi.json");
        assert_eq!(parsed.line_num, 2427);
        assert_eq!(parsed.content, "        \"summary\": \"Get a User\",");
        assert!(!parsed.is_match);
    }

    #[test]
    fn test_parse_context_unknown_file_returns_none() {
        let known = HashSet::new();
        assert!(parse_context_line("src/main.rs-10-content", &known).is_none());
    }

    #[test]
    fn test_block_separator_skipped() {
        // -- lines should not appear in output
        let mut lines = Vec::new();
        for i in 1..=5 {
            lines.push(format!("src/a.rs:{i}:match {i}\nsrc/a.rs-{}-ctx", i + 100));
        }
        let input = lines.join("\n--\n");
        let result = compact_grep(&input).unwrap();
        assert!(!result.contains("--"));
    }

    // ── match lines always kept when truncating ────────────────────

    #[test]
    fn test_grep_r_b_match_not_lost() {
        // grep -rB 30: match is the LAST line in the block, preceded by 30 context lines.
        // The match must still appear in output even when context exceeds MAX_MATCHES_PER_FILE.
        let mut block_lines = Vec::new();
        // 30 context lines before the match
        for i in 1..=30 {
            block_lines.push(format!(
                "edgee-cli/openapi/openapi.json-        \"field{}\": \"value{}\",",
                i, i
            ));
        }
        // The actual match line
        block_lines.push(
            "edgee-cli/openapi/openapi.json:        \"operationId\": \"deleteInvitation\","
                .to_string(),
        );
        let input = block_lines.join("\n");
        let result = compact_grep(&input).unwrap();
        // The match line must be in the output
        assert!(
            result.contains("\"operationId\": \"deleteInvitation\","),
            "match line must not be truncated, got:\n{}",
            result
        );
        assert!(result.contains("edgee-cli/openapi/openapi.json"));
    }

    #[test]
    fn test_grep_c_match_centered() {
        // grep -rnC 30: match is in the MIDDLE of the block (30 before, 30 after).
        // The match must appear in output and the window must be centered (roughly
        // equal context before and after), not just the top context lines.
        let file = "edgee-cli/openapi/openapi.json";
        let mut block_lines = Vec::new();
        // 30 context lines before match (line numbers 2396..=2425)
        for i in 2396usize..=2425 {
            block_lines.push(format!("{file}-{i}-  \"field{i}\": \"value\","));
        }
        // The actual match line at 2426
        block_lines.push(format!("{file}:2426:        \"operationId\": \"getMe\","));
        // 30 context lines after match (line numbers 2427..=2456)
        for i in 2427usize..=2456 {
            block_lines.push(format!("{file}-{i}-  \"field{i}\": \"value\","));
        }
        let input = block_lines.join("\n");
        let result = super::compact_grep(
            &input,
            &GrepCommandInfo {
                single_file: None,
                recursive_single_target: None,
                has_line_numbers: true,
                context_lines: 30,
            },
        )
        .unwrap();

        // The match line must be present
        assert!(
            result.contains("\"operationId\": \"getMe\","),
            "match line must not be truncated, got:\n{}",
            result
        );

        // The window must be roughly centered: there should be context lines
        // both before AND after the match, not just before.
        assert!(
            result.contains("2427"),
            "should show at least one after-context line (2427), got:\n{}",
            result
        );
        assert!(
            result.contains("2425"),
            "should show at least one before-context line (2425), got:\n{}",
            result
        );
    }

    #[test]
    fn test_select_lines_prioritizes_matches() {
        // 20 context lines then 1 match — match must survive truncation
        let mut entries: Vec<(usize, String, bool)> = Vec::new();
        for i in 1..=20 {
            entries.push((i, format!("context line {}", i), false));
        }
        entries.push((21, "THE MATCH".to_string(), true));

        let selected = select_lines(&entries, 10);
        assert!(
            selected.iter().any(|(_, c, m)| *m && c == "THE MATCH"),
            "match line must be in selected lines"
        );
        assert!(selected.len() <= 10);
    }

    #[test]
    fn test_single_file_grep_with_line_numbers() {
        // Single file grep without -r outputs: linenum:content
        // Should prepend filename when compressing
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
        let result = super::compact_grep(input, &single_file_info("main.rs", true)).unwrap();
        assert!(result.contains("main.rs"));
        assert!(result.contains("12 in 1F:"));
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn test_single_file_with_context() {
        // Single file grep -A2 outputs: linenum:content, linenum-context, --
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
        let result = super::compact_grep(input, &single_file_info("file.rs", true)).unwrap();
        assert!(result.contains("file.rs"));
        assert!(result.contains("12 in 1F:"));
        assert!(result.contains("10: fn main()"));
        assert!(result.contains("11- ") && result.contains("let x"));
    }

    // ── parse_grep_command ───────────────────────────────────────────────────

    #[test]
    fn test_parse_grep_command_dump() {
        let commands = [
            r#"grep -rA 30 '"operationId": "getMe"' edgee-cli/"#,
            r#"grep -rnA 30 '"operationId": "getMe"' edgee-cli/"#,
            r#"grep -rnA 30 '"operationId": "getMe"' edgee-cli/openapi/openapi.json"#,
            r#"grep -rA 30 '"operationId": "getMe"' edgee-cli/openapi/openapi.json"#,
            r#"grep -n 'pattern' src/main.rs"#,
            r#"grep -rn 'pattern' src/"#,
            r#"git log | grep -n 'fix'"#,
            r#"grep -A5 'pattern' file.txt | head -20"#,
        ];
        for cmd in &commands {
            let info = parse_grep_command(cmd);
            println!("command:  {:?}", cmd);
            println!("parsed:   {:?}", info);
            println!();
        }
    }

    // ── real-world: grep -A 30 '"operationId": "getMe"' edgee-cli/ ─────────

    #[test]
    fn test_real_grep_r_a_multi_file() {
        // grep -rA 30 '"operationId": "getMe"' edgee-cli/
        // Multi-file, no line numbers, dashed filenames, large context block
        let input = "\
edgee-cli/openapi/openapi2.json:\"operationId\": \"getMe\",
--
edgee-cli/openapi/openapi.json:        \"operationId\": \"getMe\",
edgee-cli/openapi/openapi.json-        \"summary\": \"Get my User object\",
edgee-cli/openapi/openapi.json-        \"description\": \"Retrieves my current User object.\",
edgee-cli/openapi/openapi.json-        \"responses\": {
edgee-cli/openapi/openapi.json-          \"200\": {
edgee-cli/openapi/openapi.json-            \"description\": \"Your User object\",
edgee-cli/openapi/openapi.json-            \"content\": {
edgee-cli/openapi/openapi.json-              \"application/json\": {
edgee-cli/openapi/openapi.json-                \"schema\": {
edgee-cli/openapi/openapi.json-                  \"$ref\": \"#/components/schemas/UserWithRoles\"
edgee-cli/openapi/openapi.json-                }
edgee-cli/openapi/openapi.json-              }
edgee-cli/openapi/openapi.json-            }
edgee-cli/openapi/openapi.json-          },
edgee-cli/openapi/openapi.json-          \"4XX\": {
edgee-cli/openapi/openapi.json-            \"description\": \"unexpected error\",
edgee-cli/openapi/openapi.json-            \"content\": {
edgee-cli/openapi/openapi.json-              \"application/json\": {
edgee-cli/openapi/openapi.json-                \"schema\": {
edgee-cli/openapi/openapi.json-                  \"$ref\": \"#/components/schemas/ErrorResponse\"
edgee-cli/openapi/openapi.json-                }
edgee-cli/openapi/openapi.json-              }
edgee-cli/openapi/openapi.json-            }
edgee-cli/openapi/openapi.json-          }
edgee-cli/openapi/openapi.json-        }
edgee-cli/openapi/openapi.json-      }
edgee-cli/openapi/openapi.json-    },
edgee-cli/openapi/openapi.json-    \"/v1/users/{id}\": {
edgee-cli/openapi/openapi.json-      \"get\": {
edgee-cli/openapi/openapi.json-        \"operationId\": \"getUser\",
edgee-cli/openapi/openapi.json-        \"summary\": \"Get a User\",
";
        let result = compact_grep(input).unwrap();
        assert!(
            result.contains("edgee-cli/openapi/openapi2.json"),
            "openapi2.json missing:\n{}",
            result
        );
        assert!(
            result.contains("edgee-cli/openapi/openapi.json"),
            "openapi.json missing:\n{}",
            result
        );
        assert!(
            result.contains("\"operationId\": \"getMe\","),
            "match content missing:\n{}",
            result
        );
        assert!(!result.contains("--"), "-- separator leaked:\n{}", result);
        assert!(!result.contains("json-"), "mangled filename:\n{}", result);
    }

    #[test]
    fn test_real_grep_rn_a_multi_file() {
        // grep -rnA 30 '"operationId": "getMe"' edgee-cli/
        // Multi-file, with line numbers, dashed filenames, large context block
        let input = "\
edgee-cli/openapi/openapi2.json:1:\"operationId\": \"getMe\",
--
edgee-cli/openapi/openapi.json:2426:        \"operationId\": \"getMe\",
edgee-cli/openapi/openapi.json-2427-        \"summary\": \"Get my User object\",
edgee-cli/openapi/openapi.json-2428-        \"description\": \"Retrieves my current User object.\",
edgee-cli/openapi/openapi.json-2429-        \"responses\": {
edgee-cli/openapi/openapi.json-2430-          \"200\": {
edgee-cli/openapi/openapi.json-2431-            \"description\": \"Your User object\",
edgee-cli/openapi/openapi.json-2432-            \"content\": {
edgee-cli/openapi/openapi.json-2433-              \"application/json\": {
edgee-cli/openapi/openapi.json-2434-                \"schema\": {
edgee-cli/openapi/openapi.json-2435-                  \"$ref\": \"#/components/schemas/UserWithRoles\"
edgee-cli/openapi/openapi.json-2436-                }
edgee-cli/openapi/openapi.json-2437-              }
edgee-cli/openapi/openapi.json-2438-            }
edgee-cli/openapi/openapi.json-2439-          },
edgee-cli/openapi/openapi.json-2440-          \"4XX\": {
edgee-cli/openapi/openapi.json-2441-            \"description\": \"unexpected error\",
edgee-cli/openapi/openapi.json-2442-            \"content\": {
edgee-cli/openapi/openapi.json-2443-              \"application/json\": {
edgee-cli/openapi/openapi.json-2444-                \"schema\": {
edgee-cli/openapi/openapi.json-2445-                  \"$ref\": \"#/components/schemas/ErrorResponse\"
edgee-cli/openapi/openapi.json-2446-                }
edgee-cli/openapi/openapi.json-2447-              }
edgee-cli/openapi/openapi.json-2448-            }
edgee-cli/openapi/openapi.json-2449-          }
edgee-cli/openapi/openapi.json-2450-        }
edgee-cli/openapi/openapi.json-2451-      }
edgee-cli/openapi/openapi.json-2452-    },
edgee-cli/openapi/openapi.json-2453-    \"/v1/users/{id}\": {
edgee-cli/openapi/openapi.json-2454-      \"get\": {
edgee-cli/openapi/openapi.json-2455-        \"operationId\": \"getUser\",
edgee-cli/openapi/openapi.json-2456-        \"summary\": \"Get a User\",
";
        let result = compact_grep(input).unwrap();
        assert!(
            result.contains("edgee-cli/openapi/openapi2.json"),
            "openapi2.json missing:\n{}",
            result
        );
        assert!(
            result.contains("edgee-cli/openapi/openapi.json"),
            "openapi.json missing:\n{}",
            result
        );
        assert!(
            result.contains("2426:"),
            "match line number missing:\n{}",
            result
        );
        assert!(
            result.contains("\"operationId\": \"getMe\","),
            "match content missing:\n{}",
            result
        );
        assert!(!result.contains("--"), "-- separator leaked:\n{}", result);
        assert!(!result.contains("json-"), "mangled filename:\n{}", result);
    }

    #[test]
    fn test_real_grep_rn_a_single_file() {
        // grep -rnA 30 '"operationId": "getMe"' edgee-cli/openapi/openapi.json
        // Single file, with line numbers, no filename prefix in output
        let input = "\
2426:        \"operationId\": \"getMe\",
2427-        \"summary\": \"Get my User object\",
2428-        \"description\": \"Retrieves my current User object.\",
2429-        \"responses\": {
2430-          \"200\": {
2431-            \"description\": \"Your User object\",
2432-            \"content\": {
2433-              \"application/json\": {
2434-                \"schema\": {
2435-                  \"$ref\": \"#/components/schemas/UserWithRoles\"
2436-                }
2437-              }
2438-            }
2439-          },
2440-          \"4XX\": {
2441-            \"description\": \"unexpected error\",
2442-            \"content\": {
2443-              \"application/json\": {
2444-                \"schema\": {
2445-                  \"$ref\": \"#/components/schemas/ErrorResponse\"
2446-                }
2447-              }
2448-            }
2449-          }
2450-        }
2451-      }
2452-    },
2453-    \"/v1/users/{id}\": {
2454-      \"get\": {
2455-        \"operationId\": \"getUser\",
2456-        \"summary\": \"Get a User\",
";
        let result = super::compact_grep(
            input,
            &single_file_info("edgee-cli/openapi/openapi.json", true),
        )
        .unwrap();
        assert!(
            result.contains("edgee-cli/openapi/openapi.json"),
            "filename missing:\n{}",
            result
        );
        assert!(
            result.contains("2426:"),
            "match line number missing:\n{}",
            result
        );
        assert!(
            result.contains("\"operationId\": \"getMe\","),
            "match content missing:\n{}",
            result
        );
        // Context lines should appear with dash separator, not with line-num embedded in content
        assert!(
            result.contains("2427-"),
            "context line number missing:\n{}",
            result
        );
    }

    #[test]
    fn test_real_grep_r_a_single_file_no_linenums() {
        // grep -rA 30 '"operationId": "getMe"' edgee-cli/openapi/openapi.json
        // Single file, no line numbers, no filename prefix in output (bare content)
        let input = "\
        \"operationId\": \"getMe\",
        \"summary\": \"Get my User object\",
        \"description\": \"Retrieves my current User object.\",
        \"responses\": {
          \"200\": {
            \"description\": \"Your User object\",
            \"content\": {
              \"application/json\": {
                \"schema\": {
                  \"$ref\": \"#/components/schemas/UserWithRoles\"
                }
              }
            }
          },
          \"4XX\": {
            \"description\": \"unexpected error\",
            \"content\": {
              \"application/json\": {
                \"schema\": {
                  \"$ref\": \"#/components/schemas/ErrorResponse\"
                }
              }
            }
          }
        }
      }
    },
    \"/v1/users/{id}\": {
      \"get\": {
        \"operationId\": \"getUser\",
        \"summary\": \"Get a User\",
";
        let result = super::compact_grep(
            input,
            &single_file_info("edgee-cli/openapi/openapi.json", false),
        )
        .unwrap();
        assert!(
            result.contains("edgee-cli/openapi/openapi.json"),
            "filename missing:\n{}",
            result
        );
        assert!(
            result.contains("\"operationId\": \"getMe\","),
            "match content missing:\n{}",
            result
        );
    }

    #[test]
    fn test_real_grep_n_b_single_file_json() {
        // Reproduces: grep -nB 30 getMe openapi.json
        // Single file, line numbers, -B only (match is LAST line)
        let input = "\
2396-              \"type\": \"string\"
2397-            }
2398-          }
2399-        ],
2400-        \"responses\": {
2401-          \"200\": {
2402-            \"description\": \"The deleted Invitation\",
2403-            \"content\": {
2404-              \"application/json\": {
2405-                \"schema\": {
2406-                  \"$ref\": \"#/components/schemas/DeletedResponse\"
2407-                }
2408-              }
2409-            }
2410-          },
2411-          \"4XX\": {
2412-            \"description\": \"unexpected error\",
2413-            \"content\": {
2414-              \"application/json\": {
2415-                \"schema\": {
2416-                  \"$ref\": \"#/components/schemas/ErrorResponse\"
2417-                }
2418-              }
2419-            }
2420-          }
2421-        }
2422-      }
2423-    },
2424-    \"/v1/users/me\": {
2425-      \"get\": {
2426:        \"operationId\": \"getMe\",
";
        let info = GrepCommandInfo {
            single_file: Some("openapi.json".to_string()),
            recursive_single_target: None,
            has_line_numbers: true,
            context_lines: 30,
        };
        let result = super::compact_grep(input, &info).unwrap();
        eprintln!("RESULT:\n{}", result);
        assert!(
            result.contains("\"operationId\": \"getMe\","),
            "match line must appear in output, got:\n{}",
            result
        );
    }

    #[test]
    fn test_real_grep_n_b_single_file_json_no_context_info() {
        // Same data but with context_lines=0 (simulates parse failure)
        let input = "\
2396-              \"type\": \"string\"
2397-            }
2398-          }
2399-        ],
2400-        \"responses\": {
2401-          \"200\": {
2402-            \"description\": \"The deleted Invitation\",
2403-            \"content\": {
2404-              \"application/json\": {
2405-                \"schema\": {
2406-                  \"$ref\": \"#/components/schemas/DeletedResponse\"
2407-                }
2408-              }
2409-            }
2410-          },
2411-          \"4XX\": {
2412-            \"description\": \"unexpected error\",
2413-            \"content\": {
2414-              \"application/json\": {
2415-                \"schema\": {
2416-                  \"$ref\": \"#/components/schemas/ErrorResponse\"
2417-                }
2418-              }
2419-            }
2420-          }
2421-        }
2422-      }
2423-    },
2424-    \"/v1/users/me\": {
2425-      \"get\": {
2426:        \"operationId\": \"getMe\",
";
        let info = GrepCommandInfo {
            single_file: Some("openapi.json".to_string()),
            recursive_single_target: None,
            has_line_numbers: true,
            context_lines: 0,
        };
        let result = super::compact_grep(input, &info).unwrap();
        eprintln!("RESULT (context_lines=0):\n{}", result);
        assert!(
            result.contains("\"operationId\": \"getMe\","),
            "match line must appear in output even with context_lines=0, got:\n{}",
            result
        );
    }
}
