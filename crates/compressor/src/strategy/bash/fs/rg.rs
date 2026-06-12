//! Compressor for `rg` (ripgrep) command output.
//!
//! Handles:
//! - match output: file:line:content or file:line:col:content
//! - context output: file-line-content (and single-file line-content)
//! - --heading output: file headings with line:content entries
//! - --files-with-matches / -l output: file lists (compressed like find)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use super::BashCompressor;

const MAX_LINE_LEN: usize = 120;
const MAX_MATCHES_PER_FILE: usize = 10;
const MIN_LINES_FOR_COMPRESSION: usize = 10;

pub struct RgCompressor;

impl BashCompressor for RgCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return None;
        }

        let info = parse_rg_command(command);
        if info.files_only {
            return compress_file_list(trimmed);
        }

        compact_rg(trimmed)
    }
}

#[derive(Debug)]
struct RgCommandInfo {
    files_only: bool,
}

fn parse_rg_command(command: &str) -> RgCommandInfo {
    let tokens = shell_tokenize(command);
    let mut files_only = false;

    let mut after_dashdash = false;
    for tok in tokens {
        if after_dashdash {
            continue;
        }
        if tok == "--" {
            after_dashdash = true;
            continue;
        }
        if tok == "-l" || tok == "--files-with-matches" || tok == "--files" {
            files_only = true;
        }
        if tok.starts_with('-') && tok.len() > 2 && tok.starts_with("-l") {
            // Combined short flags (e.g., -lS)
            files_only = true;
        }
    }

    RgCommandInfo { files_only }
}

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

fn compress_file_list(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < MIN_LINES_FOR_COMPRESSION {
        return None;
    }

    Some(compact_files(&lines))
}

fn compact_files(paths: &[&str]) -> String {
    let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut by_ext: HashMap<String, usize> = HashMap::new();

    for path in paths {
        let p = Path::new(path);
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

    let total = paths.len();
    let mut out = format!("{}F {}D:\n\n", total, dirs.len());

    let mut shown = 0;
    let max_results = 50;

    for dir in &dirs {
        if shown >= max_results {
            break;
        }

        let files_in_dir = &by_dir[dir];
        let dir_display = compact_path(dir);

        let remaining = max_results - shown;
        if files_in_dir.len() <= remaining {
            out.push_str(&format!("{}/ {}\n", dir_display, files_in_dir.join(" ")));
            shown += files_in_dir.len();
        } else {
            let partial: Vec<&str> = files_in_dir.iter().take(remaining).copied().collect();
            out.push_str(&format!("{}/ {}\n", dir_display, partial.join(" ")));
            shown += partial.len();
            break;
        }
    }

    if shown < total {
        out.push_str(&format!("+{} more\n", total - shown));
    }

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

    out
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
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

#[derive(Clone)]
struct ParsedLine {
    file: String,
    line_num: usize,
    content: String,
    is_match: bool,
}

fn compact_rg(raw: &str) -> Option<String> {
    let mut by_file: BTreeMap<String, Vec<(usize, String, bool)>> = BTreeMap::new();
    let mut total = 0;

    let mut current_file: Option<String> = None;
    let mut known_files: HashSet<String> = HashSet::new();

    // First pass: collect match lines (with file prefix or heading context).
    for line in raw.lines() {
        if line == "--" {
            continue;
        }

        if let Some(heading) = parse_heading_line(line) {
            current_file = Some(heading);
            continue;
        }

        // In heading mode, prefer unprefixed parse so "10:content" lines stay
        // associated with the current heading file rather than being treated as
        // prefixed matches with a numeric filename.
        if let Some(ref file) = current_file
            && let Some(parsed) = parse_unprefixed_line(line, file)
        {
            if parsed.is_match {
                known_files.insert(parsed.file.clone());
                total += 1;
                by_file.entry(parsed.file).or_default().push((
                    parsed.line_num,
                    truncate_line(parsed.content.trim(), MAX_LINE_LEN),
                    true,
                ));
            }
            continue;
        }

        if let Some(parsed) = parse_prefixed_match_line(line) {
            known_files.insert(parsed.file.clone());
            total += 1;
            by_file.entry(parsed.file).or_default().push((
                parsed.line_num,
                truncate_line(parsed.content.trim(), MAX_LINE_LEN),
                true,
            ));
        }
    }

    // Second pass: collect context lines now that we know filenames.
    current_file = None;
    for line in raw.lines() {
        if line == "--" {
            continue;
        }

        // Check prefixed context lines before heading detection so that lines like
        // "src/foo.rs-3-content" are not mistakenly consumed as heading lines.
        if let Some(parsed) = parse_prefixed_context_line(line, &known_files) {
            total += 1;
            by_file.entry(parsed.file).or_default().push((
                parsed.line_num,
                truncate_line(parsed.content.trim(), MAX_LINE_LEN),
                false,
            ));
            continue;
        }

        if let Some(heading) = parse_heading_line(line) {
            current_file = Some(heading);
            continue;
        }

        if let Some(file) = current_file.clone()
            && let Some(parsed) = parse_unprefixed_line(line, &file)
            && !parsed.is_match
        {
            total += 1;
            by_file.entry(parsed.file).or_default().push((
                parsed.line_num,
                truncate_line(parsed.content.trim(), MAX_LINE_LEN),
                false,
            ));
        }
    }

    if total < MIN_LINES_FOR_COMPRESSION {
        return None;
    }

    let mut out = format!("{} in {}F:\n\n", total, by_file.len());

    for (file, matches) in &by_file {
        let file_display = compact_path(file);
        out.push_str(&format!("{} ({}):\n", file_display, matches.len()));

        let selected = select_lines(matches, MAX_MATCHES_PER_FILE);
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

fn parse_heading_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "--" {
        return None;
    }
    let first = trimmed.as_bytes().first().copied();
    let starts_with_digit = first.map(|b| b.is_ascii_digit()).unwrap_or(false);
    if starts_with_digit {
        return None;
    }
    if trimmed.contains(':') {
        return None;
    }
    Some(trimmed.to_string())
}

fn parse_prefixed_match_line(line: &str) -> Option<ParsedLine> {
    let parts: Vec<&str> = line.splitn(4, ':').collect();
    if parts.len() >= 3 {
        if let Ok(ln) = parts[1].trim().parse::<usize>() {
            if parts.len() == 4 && parts[2].trim().parse::<usize>().is_ok() {
                return Some(ParsedLine {
                    file: parts[0].to_string(),
                    line_num: ln,
                    content: parts[3].to_string(),
                    is_match: true,
                });
            }
            if parts.len() == 4 {
                return Some(ParsedLine {
                    file: parts[0].to_string(),
                    line_num: ln,
                    content: format!("{}:{}", parts[2], parts[3]),
                    is_match: true,
                });
            }
            if parts.len() == 3 {
                return Some(ParsedLine {
                    file: parts[0].to_string(),
                    line_num: ln,
                    content: parts[2].to_string(),
                    is_match: true,
                });
            }
        }
        if parts.len() >= 2 {
            return Some(ParsedLine {
                file: parts[0].to_string(),
                line_num: 0,
                content: line[parts[0].len() + 1..].to_string(),
                is_match: true,
            });
        }
    }
    if parts.len() == 2 {
        return Some(ParsedLine {
            file: parts[0].to_string(),
            line_num: 0,
            content: parts[1].to_string(),
            is_match: true,
        });
    }
    None
}

fn parse_prefixed_context_line(line: &str, known_files: &HashSet<String>) -> Option<ParsedLine> {
    for file in known_files {
        let prefix = format!("{}-", file);
        if let Some(rest) = line.strip_prefix(&prefix) {
            let mut split = rest.splitn(3, '-');
            let first = split.next().unwrap_or("");
            if let Ok(ln) = first.parse::<usize>() {
                if let Some(second) = split.next() {
                    if let Some(third) = split.next() {
                        if second.parse::<usize>().is_ok() {
                            return Some(ParsedLine {
                                file: file.clone(),
                                line_num: ln,
                                content: third.to_string(),
                                is_match: false,
                            });
                        }
                        return Some(ParsedLine {
                            file: file.clone(),
                            line_num: ln,
                            content: format!("{}-{}", second, third),
                            is_match: false,
                        });
                    }
                    return Some(ParsedLine {
                        file: file.clone(),
                        line_num: ln,
                        content: second.to_string(),
                        is_match: false,
                    });
                }
                return Some(ParsedLine {
                    file: file.clone(),
                    line_num: ln,
                    content: String::new(),
                    is_match: false,
                });
            }
            return Some(ParsedLine {
                file: file.clone(),
                line_num: 0,
                content: rest.to_string(),
                is_match: false,
            });
        }
    }
    None
}

fn parse_unprefixed_line(line: &str, file: &str) -> Option<ParsedLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.as_bytes().first().copied();
    let starts_with_digit = first.map(|b| b.is_ascii_digit()).unwrap_or(false);
    if !starts_with_digit {
        return None;
    }

    if let Some(colon_pos) = trimmed.find(':') {
        let (left, rest) = trimmed.split_at(colon_pos);
        if let Ok(ln) = left.parse::<usize>() {
            let rest = &rest[1..];
            if let Some(next_colon) = rest.find(':') {
                let (maybe_col, content) = rest.split_at(next_colon);
                if maybe_col.parse::<usize>().is_ok() {
                    return Some(ParsedLine {
                        file: file.to_string(),
                        line_num: ln,
                        content: content[1..].to_string(),
                        is_match: true,
                    });
                }
            }
            return Some(ParsedLine {
                file: file.to_string(),
                line_num: ln,
                content: rest.to_string(),
                is_match: true,
            });
        }
    }

    if let Some(dash_pos) = trimmed.find('-') {
        let (left, rest) = trimmed.split_at(dash_pos);
        if let Ok(ln) = left.parse::<usize>() {
            return Some(ParsedLine {
                file: file.to_string(),
                line_num: ln,
                content: rest[1..].to_string(),
                is_match: false,
            });
        }
    }

    None
}

fn select_lines(matches: &[(usize, String, bool)], max: usize) -> Vec<(usize, String, bool)> {
    if matches.len() <= max {
        return matches.to_vec();
    }

    let match_lines: Vec<_> = matches
        .iter()
        .filter(|(_, _, is_match)| *is_match)
        .collect();
    if match_lines.len() >= max {
        return match_lines.into_iter().take(max).cloned().collect();
    }

    let context_budget = max - match_lines.len();
    let mut selected: Vec<(usize, String, bool)> = Vec::new();

    for entry in match_lines {
        selected.push(entry.clone());
    }

    for (added, entry) in matches
        .iter()
        .filter(|(_, _, is_match)| !*is_match)
        .enumerate()
    {
        if added >= context_budget {
            break;
        }
        selected.push(entry.clone());
    }

    selected
}

fn truncate_line(line: &str, max_len: usize) -> String {
    if line.len() <= max_len {
        line.to_string()
    } else {
        let end = max_len.saturating_sub(3);
        let end = line.floor_char_boundary(end);
        format!("{}...", &line[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_files_with_matches_compressed() {
        let input = (0..12)
            .map(|i| format!("src/dir/file{}.rs", i))
            .collect::<Vec<_>>()
            .join("\n");
        let compressor = RgCompressor;
        let result = compressor
            .compress("rg --files-with-matches foo .", &input)
            .unwrap();
        assert!(result.contains("12F"));
        assert!(result.contains("src/dir/"));
    }

    #[test]
    fn test_heading_output() {
        let input = "\
src/main.rs
10:fn main() {
11:    println!(\"hi\");
12:    println!(\"hi2\");
13:    println!(\"hi3\");
14:    println!(\"hi4\");
src/lib.rs
3:pub fn lib() {
4:    println!(\"lib\");
5:    println!(\"lib2\");
6:    println!(\"lib3\");
7:    println!(\"lib4\");
";
        let result = compact_rg(input).unwrap();
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("src/lib.rs"));
        assert!(result.contains("10: fn main()"));
    }

    #[test]
    fn test_vimgrep_output() {
        let input = "\
src/main.rs:10:5:fn main() {
src/main.rs:11:2:println!(\"hi\");
src/lib.rs:3:1:pub fn lib() {
src/lib.rs:4:1:println!(\"lib\");
src/util.rs:1:1:fn util() {
src/util.rs:2:1:fn util2() {
src/util.rs:3:1:fn util3() {
src/util.rs:4:1:fn util4() {
src/util.rs:5:1:fn util5() {
src/util.rs:6:1:fn util6() {
";
        let result = compact_rg(input).unwrap();
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("10: fn main()"));
    }

    #[test]
    fn test_standard_prefixed_output() {
        // Standard rg output: file:line:content (no column numbers)
        let input = "\
core/src/compression/strategy/bash/rg.rs:12:use super::BashCompressor;
core/src/compression/strategy/bash/rg.rs:20:impl BashCompressor for RgCompressor {
core/src/compression/strategy/bash/rg.rs:25:    fn compress(&self, command: &str, output: &str) -> Option<String> {
core/src/compression/strategy/bash/rg.rs:30:        compact_rg(trimmed)
core/src/compression/strategy/bash/mod.rs:10:pub trait BashCompressor {
core/src/compression/strategy/bash/mod.rs:11:    fn compress(&self, command: &str, output: &str) -> Option<String>;
core/src/compression/strategy/bash/mod.rs:20:pub struct BashCompressorRegistry;
core/src/compression/strategy/bash/mod.rs:38:    fn get(&self, name: &str) -> Option<&dyn BashCompressor> {
core/src/compression/strategy/bash/curl.rs:6:use super::BashCompressor;
core/src/compression/strategy/bash/curl.rs:13:impl BashCompressor for CurlCompressor {
";
        let result = compact_rg(input).unwrap();
        assert!(result.contains("10 in 3F:"));
        assert!(result.contains("core/src/compression/strategy/bash/rg.rs"));
        assert!(result.contains("core/src/compression/strategy/bash/mod.rs"));
        assert!(result.contains("12: use super::BashCompressor;"));
    }

    #[test]
    fn test_context_output() {
        // rg -C 1 output: file-line-content for context, file:line:content for matches
        let input = "\
src/a.rs-1-// preamble
src/a.rs:2:fn foo() {
src/a.rs-3-    let x = 1;
--
src/b.rs-9-// helper
src/b.rs:10:pub fn bar() {
src/b.rs-11-    return 1;
--
src/c.rs-4-// util
src/c.rs:5:fn baz() {
src/c.rs-6-    return 42;
--
src/d.rs:1:fn extra() {}
src/d.rs:2:fn extra2() {}
src/d.rs:3:fn extra3() {}
";
        let result = compact_rg(input).unwrap();
        assert!(result.contains("src/a.rs"));
        assert!(result.contains("src/b.rs"));
        // context lines use '-' separator in output
        assert!(result.contains('-'));
        // match lines use ':' separator
        assert!(result.contains(':'));
    }

    #[test]
    fn test_below_threshold_returns_none() {
        // Fewer than MIN_LINES_FOR_COMPRESSION (10) lines → no compression
        let input = "\
src/a.rs:1:use foo;
src/b.rs:2:use bar;
src/c.rs:3:use baz;
";
        let result = compact_rg(input);
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_output_returns_none() {
        let compressor = RgCompressor;
        assert!(compressor.compress("rg foo .", "").is_none());
        assert!(compressor.compress("rg foo .", "   \n  ").is_none());
    }

    #[test]
    fn test_short_l_flag_files_only() {
        // -l short flag should trigger file list compression
        let input = (0..12)
            .map(|i| format!("src/module{}/lib.rs", i))
            .collect::<Vec<_>>()
            .join("\n");
        let compressor = RgCompressor;
        let result = compressor.compress("rg -l somepattern .", &input).unwrap();
        assert!(result.contains("12F"));
    }

    #[test]
    fn test_files_flag_files_only() {
        // --files flag (list all files without searching) triggers file list path
        let input = (0..12)
            .map(|i| format!("src/dir{}/mod.rs", i))
            .collect::<Vec<_>>()
            .join("\n");
        let compressor = RgCompressor;
        let result = compressor.compress("rg --files .", &input).unwrap();
        assert!(result.contains("12F"));
    }

    #[test]
    fn test_combined_short_flags_files_only() {
        // -lS combined flags should still trigger files-only mode
        let input = (0..12)
            .map(|i| format!("src/pkg{}/main.rs", i))
            .collect::<Vec<_>>()
            .join("\n");
        let compressor = RgCompressor;
        let result = compressor.compress("rg -lS pattern src/", &input).unwrap();
        assert!(result.contains("12F"));
    }

    #[test]
    fn test_max_matches_per_file_truncated() {
        // More than MAX_MATCHES_PER_FILE (10) matches in one file — count shown but lines capped
        let input = (1..=15)
            .map(|i| format!("src/big.rs:{}:match line {}", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = compact_rg(&input).unwrap();
        // header should reflect total matches
        assert!(result.contains("15 in 1F:"));
        // file section shows true count
        assert!(result.contains("src/big.rs (15):"));
        // only MAX_MATCHES_PER_FILE (10) lines displayed (indented with leading spaces)
        let displayed = result
            .lines()
            .filter(|l| l.starts_with("  ") && !l.trim().is_empty())
            .count();
        assert_eq!(displayed, 10);
    }

    #[test]
    fn test_long_line_truncated() {
        // Lines longer than MAX_LINE_LEN (120) should be truncated with "..."
        let long_content = "x".repeat(200);
        let input = (1..=10)
            .map(|i| format!("src/long.rs:{}:{}", i, long_content))
            .collect::<Vec<_>>()
            .join("\n");
        let result = compact_rg(&input).unwrap();
        assert!(result.contains("..."));
        // No line in the output should exceed MAX_LINE_LEN significantly
        for line in result.lines() {
            assert!(line.len() <= MAX_LINE_LEN + 20); // allow for line prefix overhead
        }
    }

    #[test]
    fn test_file_list_many_files_shows_more() {
        // More than 50 files in file list should show "+X more"
        let input = (0..60)
            .map(|i| format!("src/gen/file{}.rs", i))
            .collect::<Vec<_>>()
            .join("\n");
        let compressor = RgCompressor;
        let result = compressor
            .compress("rg --files-with-matches foo .", &input)
            .unwrap();
        assert!(result.contains("60F"));
        assert!(result.contains("+10 more"));
    }

    #[test]
    fn test_file_list_multiple_extensions() {
        // File list with multiple extensions shows extension summary
        let mut files: Vec<String> = (0..6).map(|i| format!("src/file{}.rs", i)).collect();
        files.extend((0..4).map(|i| format!("tests/test{}.py", i)));
        let input = files.join("\n");
        let compressor = RgCompressor;
        let result = compressor.compress("rg -l pattern .", &input).unwrap();
        assert!(result.contains(".rs(6)"));
        assert!(result.contains(".py(4)"));
    }

    #[test]
    fn test_multi_file_summary_header() {
        // Verify summary header format: "{N} in {M}F:"
        let input = "\
src/alpha.rs:1:fn alpha() {}
src/alpha.rs:2:fn alpha2() {}
src/alpha.rs:3:fn alpha3() {}
src/alpha.rs:4:fn alpha4() {}
src/alpha.rs:5:fn alpha5() {}
src/beta.rs:10:fn beta() {}
src/beta.rs:11:fn beta2() {}
src/beta.rs:12:fn beta3() {}
src/beta.rs:13:fn beta4() {}
src/beta.rs:14:fn beta5() {}
";
        let result = compact_rg(input).unwrap();
        assert!(result.starts_with("10 in 2F:"));
        assert!(result.contains("src/alpha.rs (5):"));
        assert!(result.contains("src/beta.rs (5):"));
    }
}
