//! Compressor for `git` subcommands.
//!
//! Delegates `git diff`/`git show` to `DiffCompressor`.
//! Compresses `git status` by stripping hint lines and shortening section headers.
//! Compresses `git log` by collapsing the default verbose format into one line per commit.

use super::{BashCompressor, diff::DiffCompressor};

pub struct GitCompressor;

impl BashCompressor for GitCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        match parse_git_subcommand(command) {
            "diff" | "show" => DiffCompressor.compress(command, output),
            "status" => compress_git_status(output),
            "log" => compress_git_log(command, output),
            _ => None,
        }
    }
}

fn parse_git_subcommand(command: &str) -> &str {
    // Skip flags like `-C /path` to find the real subcommand
    let mut skip_next = false;
    for arg in command.split_whitespace().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        // Flags that consume the next token
        if matches!(arg, "-C" | "--git-dir" | "--work-tree" | "--namespace") {
            skip_next = true;
            continue;
        }
        if !arg.starts_with('-') {
            return arg;
        }
    }
    ""
}

fn compress_git_status(output: &str) -> Option<String> {
    let original_len = output.trim_end().len();
    let mut result: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip blank lines — git status sections are self-separating
        if trimmed.is_empty() {
            continue;
        }

        // Strip parenthesized hint lines ("use 'git add'...", "use 'git restore'...", etc.)
        if trimmed.starts_with('(') {
            continue;
        }

        // Rename verbose section headers to compact forms
        let mapped = match trimmed {
            "Changes to be committed:" => "Staged:",
            "Changes not staged for commit:" => "Unstaged:",
            "Untracked files:" => "Untracked:",
            other => other,
        };

        result.push(mapped.to_string());
    }

    if result.is_empty() {
        return None;
    }

    let compressed = result.join("\n");
    if compressed.len() >= original_len {
        return None;
    }

    Some(compressed)
}

fn compress_git_log(command: &str, output: &str) -> Option<String> {
    // --oneline is already compact; just cap at 50 entries
    if command.contains("--oneline") {
        let lines: Vec<&str> = output.lines().collect();
        if lines.len() <= 50 {
            return None;
        }
        let extra = lines.len() - 50;
        let mut result = lines[..50].join("\n");
        result.push_str(&format!("\n... and {extra} more commits"));
        return Some(result);
    }

    // Custom format: unknown structure, leave alone
    if command.contains("--format=")
        || command.contains("--format ")
        || command.contains("--pretty=")
        || command.contains("--pretty ")
    {
        return None;
    }

    compress_default_git_log(output)
}

/// Collapses the default multi-line git log format into one line per commit:
/// `<short-hash> <subject> (<author>, <date>)`
fn compress_default_git_log(output: &str) -> Option<String> {
    let mut commits: Vec<String> = Vec::new();

    let mut current_hash = String::new();
    let mut current_author = String::new();
    let mut current_date = String::new();
    let mut current_subject = String::new();
    let mut in_body = false;

    let flush = |commits: &mut Vec<String>, hash: &str, author: &str, date: &str, subject: &str| {
        if !hash.is_empty() && !subject.is_empty() {
            let short = &hash[..hash.len().min(7)];
            commits.push(format!("{short} {subject} ({author}, {date})"));
        }
    };

    for line in output.lines() {
        if line.starts_with("commit ") {
            flush(
                &mut commits,
                &current_hash,
                &current_author,
                &current_date,
                &current_subject,
            );
            // "commit <full-hash> [<refs>]"
            current_hash = line.split_whitespace().nth(1).unwrap_or("").to_string();
            current_author.clear();
            current_date.clear();
            current_subject.clear();
            in_body = false;
        } else if line.starts_with("Author: ") {
            let author = line.trim_start_matches("Author: ");
            // Strip email, keep only display name
            current_author = if let Some(end) = author.find(" <") {
                author[..end].to_string()
            } else {
                author.to_string()
            };
        } else if line.starts_with("Date: ") || line.starts_with("AuthorDate: ") {
            let raw = if line.starts_with("AuthorDate: ") {
                line.trim_start_matches("AuthorDate: ")
            } else {
                line.trim_start_matches("Date: ")
            };
            // Keep "Mon Jun 9" (day-of-week, month, day)
            current_date = raw.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
        } else if line.starts_with("Merge: ")
            || line.starts_with("Commit: ")
            || line.starts_with("CommitDate: ")
        {
            // Skip decoration lines
        } else if line.trim().is_empty() {
            in_body = true;
        } else if in_body && line.starts_with("    ") && current_subject.is_empty() {
            // First non-empty indented line is the commit subject
            current_subject = line.trim().to_string();
        }
    }

    flush(
        &mut commits,
        &current_hash,
        &current_author,
        &current_date,
        &current_subject,
    );

    if commits.is_empty() {
        return None;
    }

    const MAX_COMMITS: usize = 30;
    let total = commits.len();
    let mut result = commits[..total.min(MAX_COMMITS)].join("\n");
    if total > MAX_COMMITS {
        result.push_str(&format!("\n... and {} more commits", total - MAX_COMMITS));
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_git_subcommand ──────────────────────────────────────────────

    #[test]
    fn test_parse_subcommand_simple() {
        assert_eq!(parse_git_subcommand("git status"), "status");
        assert_eq!(parse_git_subcommand("git log --oneline"), "log");
        assert_eq!(parse_git_subcommand("git diff HEAD"), "diff");
    }

    #[test]
    fn test_parse_subcommand_with_git_dir_flag() {
        assert_eq!(parse_git_subcommand("git -C /some/path status"), "status");
    }

    // ── git status ────────────────────────────────────────────────────────

    const STATUS_OUTPUT: &str = "On branch main
Your branch is up to date with 'origin/main'.

Changes to be committed:
  (use \"git restore --staged <file>...\" to unstage)
\tmodified:   src/main.rs

Changes not staged for commit:
  (use \"git add <file>...\" to update what will be committed)
  (use \"git restore <file>...\" to discard changes in working directory)
\tmodified:   README.md

Untracked files:
  (use \"git add <file>...\" to include in what will be committed)
\tnew_file.txt
";

    #[test]
    fn test_status_strips_hints() {
        let result = compress_git_status(STATUS_OUTPUT).unwrap();
        assert!(!result.contains("(use "));
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("README.md"));
        assert!(result.contains("new_file.txt"));
    }

    #[test]
    fn test_status_renames_section_headers() {
        let result = compress_git_status(STATUS_OUTPUT).unwrap();
        assert!(result.contains("Staged:"));
        assert!(result.contains("Unstaged:"));
        assert!(result.contains("Untracked:"));
        assert!(!result.contains("Changes to be committed:"));
        assert!(!result.contains("Changes not staged for commit:"));
    }

    #[test]
    fn test_status_clean_passthrough() {
        let clean = "On branch main\nnothing to commit, working tree clean\n";
        // Short output with no hints — should still compress blank lines
        let result = compress_git_status(clean);
        // Either None (already compact) or Some with blank lines removed
        if let Some(r) = result {
            assert!(!r.contains("\n\n"));
        }
    }

    // ── git log ───────────────────────────────────────────────────────────

    const LOG_OUTPUT: &str = "commit abc1234defabc1234defabc1234defabc1234de
Author: Alice Smith <alice@example.com>
Date:   Mon Jun 9 10:00:00 2026 -0700

    Fix the broken thing

commit def5678abcdef5678abcdef5678abcdef5678ab
Author: Bob Jones <bob@example.com>
Date:   Sun Jun 8 15:30:00 2026 -0700

    Add new feature
";

    #[test]
    fn test_log_collapses_to_one_line_per_commit() {
        let result = compress_default_git_log(LOG_OUTPUT).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("abc1234"));
        assert!(lines[0].contains("Fix the broken thing"));
        assert!(lines[0].contains("Alice Smith"));
        assert!(lines[1].contains("def5678"));
    }

    #[test]
    fn test_log_strips_email() {
        let result = compress_default_git_log(LOG_OUTPUT).unwrap();
        assert!(!result.contains("alice@example.com"));
        assert!(result.contains("Alice Smith"));
    }

    #[test]
    fn test_log_oneline_capped_at_50() {
        let lines: Vec<String> = (0..60).map(|i| format!("abc{i:04} commit {i}")).collect();
        let output = lines.join("\n");
        let result = compress_git_log("git log --oneline", &output).unwrap();
        assert!(result.contains("... and 10 more commits"));
    }

    #[test]
    fn test_log_oneline_under_50_passthrough() {
        let lines: Vec<String> = (0..20).map(|i| format!("abc{i:04} commit {i}")).collect();
        let output = lines.join("\n");
        assert!(compress_git_log("git log --oneline", &output).is_none());
    }

    #[test]
    fn test_log_custom_format_passthrough() {
        let output = "2026-06-09 Fix thing\n2026-06-08 Add feature\n";
        assert!(compress_git_log("git log --format='%ai %s'", output).is_none());
    }

    // ── compressor dispatch ───────────────────────────────────────────────

    #[test]
    fn test_dispatch_diff_delegates() {
        let diff = "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let c = GitCompressor;
        assert!(c.compress("git diff", diff).is_some());
    }

    #[test]
    fn test_dispatch_unknown_subcommand_passthrough() {
        let c = GitCompressor;
        assert!(
            c.compress("git blame src/main.rs", "some output\n")
                .is_none()
        );
    }

    #[test]
    fn test_empty_output_passthrough() {
        let c = GitCompressor;
        assert!(c.compress("git status", "").is_none());
        assert!(c.compress("git log", "").is_none());
    }
}
