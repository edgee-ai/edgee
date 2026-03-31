//! Compressor for `pytest` command output.
//!
//! Parses pytest output to show only failures and a compact summary,
//! stripping session headers, passing test lines, and verbose output.

use super::BashCompressor;

pub struct PytestCompressor;

impl BashCompressor for PytestCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        Some(filter_pytest_output(output))
    }
}

#[derive(Debug, PartialEq)]
enum ParseState {
    Header,
    TestProgress,
    Failures,
    Summary,
}

/// Parse pytest output using state machine.
fn filter_pytest_output(output: &str) -> String {
    let mut state = ParseState::Header;
    let mut failures: Vec<String> = Vec::new();
    let mut current_failure: Vec<String> = Vec::new();
    let mut summary_line = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // State transitions
        if trimmed.starts_with("===") && trimmed.contains("test session starts") {
            state = ParseState::Header;
            continue;
        } else if trimmed.starts_with("===") && trimmed.contains("FAILURES") {
            state = ParseState::Failures;
            continue;
        } else if trimmed.starts_with("===") && trimmed.contains("short test summary") {
            state = ParseState::Summary;
            if !current_failure.is_empty() {
                failures.push(current_failure.join("\n"));
                current_failure.clear();
            }
            continue;
        } else if trimmed.starts_with("===")
            && (trimmed.contains("passed") || trimmed.contains("failed"))
        {
            summary_line = trimmed.to_string();
            continue;
        }

        match state {
            ParseState::Header => {
                if trimmed.starts_with("collected") {
                    state = ParseState::TestProgress;
                }
            }
            ParseState::TestProgress => {
                // Skip individual test lines like "tests/test_foo.py .... [100%]"
            }
            ParseState::Failures => {
                if trimmed.starts_with("___") {
                    if !current_failure.is_empty() {
                        failures.push(current_failure.join("\n"));
                        current_failure.clear();
                    }
                    current_failure.push(trimmed.to_string());
                } else if !trimmed.is_empty() && !trimmed.starts_with("===") {
                    current_failure.push(trimmed.to_string());
                }
            }
            ParseState::Summary => {
                if trimmed.starts_with("FAILED") || trimmed.starts_with("ERROR") {
                    failures.push(trimmed.to_string());
                }
            }
        }
    }

    if !current_failure.is_empty() {
        failures.push(current_failure.join("\n"));
    }

    build_pytest_summary(&summary_line, &failures)
}

fn build_pytest_summary(summary: &str, failures: &[String]) -> String {
    let (passed, failed, skipped) = parse_summary_line(summary);

    if failed == 0 && passed > 0 {
        return format!("Pytest: {} passed", passed);
    }

    if passed == 0 && failed == 0 {
        return "Pytest: No tests collected".to_string();
    }

    let mut result = format!("Pytest: {} passed, {} failed", passed, failed);
    if skipped > 0 {
        result.push_str(&format!(", {} skipped", skipped));
    }
    result.push('\n');

    if failures.is_empty() {
        return result.trim().to_string();
    }

    result.push_str("\nFailures:\n");

    for (i, failure) in failures.iter().take(5).enumerate() {
        let lines: Vec<&str> = failure.lines().collect();

        if let Some(first_line) = lines.first() {
            if first_line.starts_with("___") {
                let test_name = first_line.trim_matches('_').trim();
                result.push_str(&format!("{}. {}\n", i + 1, test_name));
            } else if first_line.starts_with("FAILED") {
                let parts: Vec<&str> = first_line.split(" - ").collect();
                if let Some(test_path) = parts.first() {
                    let test_name = test_path.trim_start_matches("FAILED ");
                    result.push_str(&format!("{}. {}\n", i + 1, test_name));
                }
                if parts.len() > 1 {
                    result.push_str(&format!("     {}\n", truncate(parts[1], 100)));
                }
                continue;
            }
        }

        // Show relevant error lines (assertions, errors, file locations)
        let mut relevant_lines = 0;
        for line in lines.iter().skip(1) {
            let line_lower = line.to_lowercase();
            let is_relevant = line.trim().starts_with('>')
                || line.trim().starts_with('E')
                || line_lower.contains("assert")
                || line_lower.contains("error")
                || line.contains(".py:");

            if is_relevant && relevant_lines < 3 {
                result.push_str(&format!("     {}\n", truncate(line, 100)));
                relevant_lines += 1;
            }
        }
    }

    if failures.len() > 5 {
        result.push_str(&format!("\n... +{} more failures\n", failures.len() - 5));
    }

    result.trim().to_string()
}

fn parse_summary_line(summary: &str) -> (usize, usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    for part in summary.split(',') {
        let words: Vec<&str> = part.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                if word.contains("passed") {
                    if let Ok(n) = words[i - 1].parse::<usize>() {
                        passed = n;
                    }
                } else if word.contains("failed") {
                    if let Ok(n) = words[i - 1].parse::<usize>() {
                        failed = n;
                    }
                } else if word.contains("skipped")
                    && let Ok(n) = words[i - 1].parse::<usize>()
                {
                    skipped = n;
                }
            }
        }
    }

    (passed, failed, skipped)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..s.floor_char_boundary(max.saturating_sub(3))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_pytest_all_pass() {
        let output = "=== test session starts ===\nplatform darwin -- Python 3.11.0\ncollected 5 items\n\ntests/test_foo.py .....                                            [100%]\n\n=== 5 passed in 0.50s ===";
        let result = filter_pytest_output(output);
        assert!(result.contains("Pytest"));
        assert!(result.contains("5 passed"));
    }

    #[test]
    fn test_filter_pytest_with_failures() {
        let output = "=== test session starts ===\ncollected 5 items\n\ntests/test_foo.py ..F..                                            [100%]\n\n=== FAILURES ===\n___ test_something ___\n\n    def test_something():\n>       assert False\nE       assert False\n\ntests/test_foo.py:10: AssertionError\n\n=== short test summary info ===\nFAILED tests/test_foo.py::test_something - assert False\n=== 4 passed, 1 failed in 0.50s ===";
        let result = filter_pytest_output(output);
        assert!(result.contains("4 passed, 1 failed"));
        assert!(result.contains("test_something"));
        assert!(result.contains("assert False"));
    }

    #[test]
    fn test_filter_pytest_no_tests() {
        let output =
            "=== test session starts ===\ncollected 0 items\n\n=== no tests ran in 0.00s ===";
        let result = filter_pytest_output(output);
        assert!(result.contains("No tests collected"));
    }

    #[test]
    fn test_parse_summary_line() {
        assert_eq!(parse_summary_line("=== 5 passed in 0.50s ==="), (5, 0, 0));
        assert_eq!(
            parse_summary_line("=== 4 passed, 1 failed in 0.50s ==="),
            (4, 1, 0)
        );
        assert_eq!(
            parse_summary_line("=== 3 passed, 1 failed, 2 skipped in 1.0s ==="),
            (3, 1, 2)
        );
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world!", 8), "hello...");
    }
}
