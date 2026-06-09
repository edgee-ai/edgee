//! Compressor for `gh` (GitHub CLI) output.
//!
//! Strips the "Showing N of M..." preamble, the column-header row, and
//! collapses multi-space padding between columns so each row reads cleanly.

use super::BashCompressor;

pub struct GhCompressor;

impl BashCompressor for GhCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let result = compress_gh_output(output);
        if result.trim() == output.trim() {
            return None;
        }

        Some(result)
    }
}

fn is_column_header_row(line: &str) -> bool {
    // gh table headers are SCREAMING_CASE or Title Case column names
    // e.g. "  #    TITLE           BRANCH      CREATED AT"
    //      "STATUS  TITLE       WORKFLOW  BRANCH  EVENT  ID  ELAPSED  AGE"
    let upper = line.to_uppercase();
    upper == line.to_ascii_uppercase()
        && (line.contains("TITLE")
            || line.contains("STATUS")
            || line.contains("BRANCH")
            || line.contains("WORKFLOW")
            || line.contains("CREATED")
            || line.contains("ELAPSED"))
}

fn compress_gh_output(output: &str) -> String {
    let mut result: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip the "Showing N of M..." preamble
        if trimmed.starts_with("Showing ") && trimmed.contains(" in ") {
            continue;
        }

        // Skip blank lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip column header rows
        if is_column_header_row(line) {
            continue;
        }

        // Normalize multiple spaces between columns to a single space,
        // keeping leading indentation for PR/issue number alignment.
        let normalized = normalize_columns(line);
        result.push(normalized);
    }

    result.join("\n")
}

/// Replace runs of 2+ spaces that appear between non-space content with a
/// single space, while preserving any leading indentation.
fn normalize_columns(line: &str) -> String {
    let leading_spaces = line.len() - line.trim_start().len();
    let indent = &line[..leading_spaces];
    let body = line.trim_start();

    let mut out = String::with_capacity(line.len());
    out.push_str(indent);

    let mut prev_space = false;
    for ch in body.chars() {
        if ch == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PR_LIST: &str = "Showing 30 of 45 open pull requests in owner/repo

  #    TITLE                          BRANCH              CREATED AT
  103  Add support for new compressors  feat/more-comp      about 2 hours ago
  102  Fix auth token refresh           fix/auth-refresh    about 3 days ago
";

    #[test]
    fn test_strips_preamble() {
        let result = GhCompressor.compress("gh pr list", PR_LIST).unwrap();
        assert!(!result.contains("Showing 30 of 45"));
    }

    #[test]
    fn test_strips_column_headers() {
        let result = GhCompressor.compress("gh pr list", PR_LIST).unwrap();
        assert!(!result.contains("TITLE"));
        assert!(!result.contains("BRANCH"));
        assert!(!result.contains("CREATED AT"));
    }

    #[test]
    fn test_keeps_data_rows() {
        let result = GhCompressor.compress("gh pr list", PR_LIST).unwrap();
        assert!(result.contains("Add support for new compressors"));
        assert!(result.contains("Fix auth token refresh"));
    }

    #[test]
    fn test_normalizes_padding() {
        let line = "  103  Add feature     feat/x   about 2h ago";
        let normalized = normalize_columns(line);
        // Leading indent preserved, internal runs collapsed
        assert!(normalized.starts_with("  "));
        assert!(!normalized.contains("     "));
    }

    #[test]
    fn test_empty_passthrough() {
        assert!(GhCompressor.compress("gh pr list", "").is_none());
    }

    #[test]
    fn test_issue_list() {
        let output = "Showing 5 of 12 open issues in owner/repo

  #    TITLE                   LABELS     UPDATED
  234  Bug: crash on startup   bug        about 1 hour ago
  233  Feature: add X          feature    about 2 days ago
";
        let result = GhCompressor.compress("gh issue list", output).unwrap();
        assert!(!result.contains("Showing"));
        assert!(result.contains("Bug: crash on startup"));
        assert!(result.contains("Feature: add X"));
    }

    #[test]
    fn test_run_list() {
        let output = "STATUS  TITLE     WORKFLOW  BRANCH  EVENT  ID          ELAPSED  AGE
✓       main      CI        main    push   1234567890  2m15s    about 1 hour ago
✗       feat/x    CI        feat/x  push   1234567891  45s      about 2 hours ago
";
        let result = GhCompressor.compress("gh run list", output).unwrap();
        assert!(!result.contains("WORKFLOW"));
        assert!(result.contains("2m15s"));
    }
}
