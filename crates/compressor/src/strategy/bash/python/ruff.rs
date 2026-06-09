//! Compressor for `ruff` (Python linter/formatter) output.
//!
//! Groups violations by file and emits a compact header per file. Strips
//! blank lines and normalises the fixable-with-`--fix` summary.

use super::BashCompressor;

pub struct RuffCompressor;

impl BashCompressor for RuffCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let compressed = compress_ruff_output(output);
        if compressed.trim() == output.trim() {
            return None;
        }

        Some(compressed)
    }
}

/// Split a ruff violation line into `(file_path, "line:col: CODE message")`.
///
/// Ruff lines look like: `src/main.py:1:1: I001 [*] Import block is un-sorted`
/// This finds the first `:digit+:digit+: ` sequence which marks the location.
fn split_ruff_violation(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            // Candidate: path ends at `i`, location starts at `i+1`
            let loc = &line[i + 1..];
            // Expect: digits ':' digits ': '
            let after_line = loc.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Some(after_colon) = after_line.strip_prefix(':') {
                let after_col = after_colon.trim_start_matches(|c: char| c.is_ascii_digit());
                if after_col.starts_with(": ") {
                    return Some((&line[..i], loc));
                }
            }
        }
        i += 1;
    }
    None
}

fn compress_ruff_output(output: &str) -> String {
    let mut file_order: Vec<String> = Vec::new();
    let mut file_violations: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut summary_lines: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Summary / footer lines
        if trimmed.starts_with("Found ")
            || trimmed.starts_with("All checks passed")
            || trimmed.starts_with("No fixes")
        {
            summary_lines.push(trimmed.to_string());
            continue;
        }
        // "[*] N fixable with the `--fix` option."
        if trimmed.starts_with('[') && trimmed.contains("fixable") {
            let normalised = trimmed
                .replace("with the `--fix` option.", "with --fix")
                .replace("[*] ", "");
            summary_lines.push(normalised);
            continue;
        }

        // Violation line
        if let Some((path, loc_and_rest)) = split_ruff_violation(trimmed) {
            let clean = loc_and_rest.replace("[*] ", "");
            if !file_violations.contains_key(path) {
                file_order.push(path.to_string());
                file_violations.insert(path.to_string(), Vec::new());
            }
            file_violations.get_mut(path).unwrap().push(clean);
        }
    }

    if file_order.is_empty() && summary_lines.is_empty() {
        return output.trim().to_string();
    }

    let mut result = String::new();
    for path in &file_order {
        let violations = &file_violations[path];
        let n = violations.len();
        result.push_str(&format!(
            "{path} ({n} {}):\n",
            if n == 1 { "issue" } else { "issues" }
        ));
        for v in violations {
            result.push_str(&format!("  {v}\n"));
        }
    }

    if !summary_lines.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&summary_lines.join("\n"));
    }

    result.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUFF_OUTPUT: &str = "src/main.py:1:1: I001 [*] Import block is un-sorted or un-formatted
src/main.py:5:1: E711 Comparison to `None` (use `is` or `is not`)
src/utils.py:10:5: F401 `os.path` imported but unused
Found 3 errors.
[*] 1 fixable with the `--fix` option.
";

    #[test]
    fn test_groups_by_file() {
        let result = RuffCompressor
            .compress("ruff check .", RUFF_OUTPUT)
            .unwrap();
        assert!(result.contains("src/main.py (2 issues):"));
        assert!(result.contains("src/utils.py (1 issue):"));
    }

    #[test]
    fn test_violations_indented_under_file() {
        let result = RuffCompressor
            .compress("ruff check .", RUFF_OUTPUT)
            .unwrap();
        // Each violation appears indented
        assert!(result.contains("  1:1: I001"));
        assert!(result.contains("  5:1: E711"));
        assert!(result.contains("  10:5: F401"));
    }

    #[test]
    fn test_strips_fixable_marker_from_violations() {
        let result = RuffCompressor
            .compress("ruff check .", RUFF_OUTPUT)
            .unwrap();
        // "[*]" should not appear in violation lines
        assert!(!result.contains("[*] Import"));
    }

    #[test]
    fn test_normalises_fixable_summary() {
        let result = RuffCompressor
            .compress("ruff check .", RUFF_OUTPUT)
            .unwrap();
        assert!(result.contains("fixable with --fix"));
        assert!(!result.contains("`--fix` option"));
    }

    #[test]
    fn test_keeps_found_summary() {
        let result = RuffCompressor
            .compress("ruff check .", RUFF_OUTPUT)
            .unwrap();
        assert!(result.contains("Found 3 errors."));
    }

    #[test]
    fn test_clean_single_line() {
        let clean = "All checks passed.\n";
        // Already one line; should either pass through as-is or return None
        let result = RuffCompressor.compress("ruff check .", clean);
        if let Some(r) = result {
            assert!(r.contains("All checks passed"));
        }
    }

    #[test]
    fn test_empty_passthrough() {
        assert!(RuffCompressor.compress("ruff check .", "").is_none());
    }

    #[test]
    fn test_split_violation_basic() {
        let (path, loc) = split_ruff_violation("src/main.py:1:1: I001 Import block").unwrap();
        assert_eq!(path, "src/main.py");
        assert_eq!(loc, "1:1: I001 Import block");
    }

    #[test]
    fn test_split_violation_nested_path() {
        let (path, loc) =
            split_ruff_violation("src/deeply/nested/module.py:42:13: E501 Line too long").unwrap();
        assert_eq!(path, "src/deeply/nested/module.py");
        assert_eq!(loc, "42:13: E501 Line too long");
    }

    #[test]
    fn test_split_violation_non_violation_returns_none() {
        assert!(split_ruff_violation("Found 3 errors.").is_none());
        assert!(split_ruff_violation("All checks passed.").is_none());
    }
}
