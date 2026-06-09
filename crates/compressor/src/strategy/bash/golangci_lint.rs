//! Compressor for `golangci-lint` output.
//!
//! Strips `level=info` log lines, timing/run lines, and decorative
//! separators, then groups violations by file.

use super::BashCompressor;

pub struct GolangciLintCompressor;

impl BashCompressor for GolangciLintCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let compressed = compress_golangci_output(output);
        if compressed.trim() == output.trim() {
            return None;
        }

        Some(compressed)
    }
}

/// A parsed golangci-lint violation line.
/// golangci-lint emits: `path:line:col: message (linter-name)`
/// or (older versions): `path:line:col: message`
struct GolangciViolation<'a> {
    path: &'a str,
    location: &'a str, // "line:col"
    rest: &'a str,     // "message (linter-name)"
}

fn parse_golangci_violation(line: &str) -> Option<GolangciViolation<'_>> {
    // Scan for the first `:<digit>` that marks the start of the location
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let path = &line[..i];
            let rest_from_line = &line[i + 1..];

            // Expect: digits ':' digits ': ' (line:col: message)
            let after_line = rest_from_line.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Some(after_colon) = after_line.strip_prefix(':') {
                let after_col = after_colon.trim_start_matches(|c: char| c.is_ascii_digit());
                if let Some(message) = after_col.strip_prefix(": ") {
                    // loc = from i+1 up to the ": " separator
                    let loc_len = rest_from_line.len() - after_col.len();
                    let location = &rest_from_line[..loc_len];
                    return Some(GolangciViolation {
                        path,
                        location,
                        rest: message,
                    });
                }
            }
        }
        i += 1;
    }
    None
}

fn is_noise_line(line: &str) -> bool {
    let trimmed = line.trim();
    // Structured log lines
    if trimmed.starts_with("level=") {
        return true;
    }
    // Decorative separator lines ("---...", "===...")
    if trimmed.len() >= 5 && trimmed.chars().all(|c| c == '-' || c == '=' || c == ' ') {
        return true;
    }
    // golangci-lint banner / version line
    if trimmed.starts_with("golangci-lint")
        && (trimmed.contains("version") || trimmed.contains("run"))
    {
        return true;
    }
    false
}

fn compress_golangci_output(output: &str) -> String {
    let mut file_order: Vec<String> = Vec::new();
    let mut file_violations: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut summary_lines: Vec<String> = Vec::new();
    let mut issue_count = 0usize;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() || is_noise_line(line) {
            continue;
        }

        if let Some(v) = parse_golangci_violation(trimmed) {
            issue_count += 1;
            if !file_violations.contains_key(v.path) {
                file_order.push(v.path.to_string());
                file_violations.insert(v.path.to_string(), Vec::new());
            }
            file_violations
                .get_mut(v.path)
                .unwrap()
                .push(format!("  {}: {}", v.location, v.rest));
        } else {
            // Keep non-noise lines that aren't violations (e.g. summary messages)
            summary_lines.push(trimmed.to_string());
        }
    }

    if file_order.is_empty() && summary_lines.is_empty() {
        return output.trim().to_string();
    }

    // If golangci-lint didn't emit its own summary, synthesise one
    let synthesise_summary = !summary_lines
        .iter()
        .any(|l| l.contains("issue") || l.contains("Issue"));

    let mut result = String::new();
    for path in &file_order {
        let violations = &file_violations[path];
        let n = violations.len();
        result.push_str(&format!(
            "{path} ({n} {}):\n",
            if n == 1 { "issue" } else { "issues" }
        ));
        for v in violations {
            result.push_str(v);
            result.push('\n');
        }
    }

    if !file_order.is_empty() && synthesise_summary {
        if issue_count == 0 {
            result.push_str("\nno issues found");
        } else {
            result.push_str(&format!("\n{issue_count} issues found"));
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

    const GOLANGCI_OUTPUT: &str = "level=info msg=\"[config] Config search path\"
level=info msg=\"Starting analysis with Go\"
pkg/service.go:15:5: undeclared name: `foo` (typecheck)
pkg/service.go:20:2: `err` is not declared in this scope (typecheck)
pkg/utils.go:8:1: exported function `Bar` should have comment or be unexported (golint)
level=info msg=\"Run finished\" duration=2.3s
";

    #[test]
    fn test_strips_log_lines() {
        let result = GolangciLintCompressor
            .compress("golangci-lint run", GOLANGCI_OUTPUT)
            .unwrap();
        assert!(!result.contains("level=info"));
    }

    #[test]
    fn test_groups_by_file() {
        let result = GolangciLintCompressor
            .compress("golangci-lint run", GOLANGCI_OUTPUT)
            .unwrap();
        assert!(result.contains("pkg/service.go (2 issues):"));
        assert!(result.contains("pkg/utils.go (1 issue):"));
    }

    #[test]
    fn test_violations_indented() {
        let result = GolangciLintCompressor
            .compress("golangci-lint run", GOLANGCI_OUTPUT)
            .unwrap();
        assert!(result.contains("  15:5: undeclared name"));
        assert!(result.contains("  8:1: exported function"));
    }

    #[test]
    fn test_empty_passthrough() {
        assert!(
            GolangciLintCompressor
                .compress("golangci-lint run", "")
                .is_none()
        );
    }

    #[test]
    fn test_no_issues() {
        let output = "level=info msg=\"Starting analysis\"
level=info msg=\"Run finished with no issues\"
";
        let result = GolangciLintCompressor.compress("golangci-lint run", output);
        // All noise lines stripped — either None or Some("no issues found")
        if let Some(r) = result {
            assert!(!r.contains("level=info"));
        }
    }

    #[test]
    fn test_parse_violation() {
        let v = parse_golangci_violation("pkg/foo.go:15:5: undeclared name (typecheck)").unwrap();
        assert_eq!(v.path, "pkg/foo.go");
        assert_eq!(v.location, "15:5");
        assert!(v.rest.contains("undeclared name"));
    }

    #[test]
    fn test_noise_detection() {
        assert!(is_noise_line("level=info msg=\"foo\""));
        assert!(is_noise_line("---------------------------------------"));
        assert!(is_noise_line("==================================="));
        assert!(!is_noise_line("pkg/foo.go:5:1: error message (linter)"));
    }
}
