//! Compressor for `go` command output.
//!
//! Filters `go test`, `go build`, and `go vet` output to show
//! only failures, errors, and compact summaries.

use std::collections::HashMap;

use super::BashCompressor;

pub struct GoCompressor;

impl BashCompressor for GoCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let subcommand = parse_go_subcommand(command);
        match subcommand {
            "test" => Some(filter_go_test(output)),
            "build" => Some(filter_go_build(output)),
            "vet" => Some(filter_go_vet(output)),
            _ => None,
        }
    }
}

fn parse_go_subcommand(command: &str) -> &str {
    for arg in command.split_whitespace().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        return arg;
    }
    ""
}

/// Filter go test text output — show failures + summary.
fn filter_go_test(output: &str) -> String {
    let mut packages: HashMap<String, PackageResult> = HashMap::new();
    let mut current_test: Option<String> = None;
    let mut current_output: Vec<String> = Vec::new();
    let mut current_package = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Package result lines: "ok  package  0.123s" or "FAIL  package  0.123s"
        if trimmed.starts_with("ok ")
            || trimmed.starts_with("FAIL\t")
            || trimmed.starts_with("ok\t")
        {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let status = parts[0];
                let package = parts[1].to_string();
                let pkg = packages.entry(package).or_default();
                if status == "FAIL" {
                    pkg.failed = true;
                }
            }
            continue;
        }

        // Test run line: "=== RUN   TestFoo"
        if trimmed.starts_with("=== RUN") {
            // Flush previous test if any
            flush_test(
                &mut packages,
                &current_package,
                &current_test,
                &current_output,
            );
            current_test = trimmed
                .strip_prefix("=== RUN")
                .map(|s| s.trim().to_string());
            current_output.clear();
            continue;
        }

        // Test result line: "--- PASS: TestFoo (0.00s)" or "--- FAIL: TestFoo (0.01s)"
        if trimmed.starts_with("--- PASS:") {
            if let Some(pkg_name) = extract_package_from_context(&current_package) {
                packages.entry(pkg_name).or_default().pass += 1;
            }
            current_test = None;
            current_output.clear();
            continue;
        }

        if trimmed.starts_with("--- FAIL:") {
            let test_name = trimmed
                .strip_prefix("--- FAIL:")
                .and_then(|s| s.split('(').next())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if let Some(pkg_name) = extract_package_from_context(&current_package) {
                let pkg = packages.entry(pkg_name).or_default();
                pkg.fail += 1;
                pkg.failed_tests.push((test_name, current_output.clone()));
            }
            current_test = None;
            current_output.clear();
            continue;
        }

        // "--- SKIP:" lines
        if trimmed.starts_with("--- SKIP:") {
            if let Some(pkg_name) = extract_package_from_context(&current_package) {
                packages.entry(pkg_name).or_default().skip += 1;
            }
            current_test = None;
            current_output.clear();
            continue;
        }

        // Package header: "# package/path"
        if let Some(stripped) = trimmed.strip_prefix('#') {
            current_package = stripped.trim().to_string();
            continue;
        }

        // Collect test output
        if current_test.is_some() && !trimmed.is_empty() {
            current_output.push(trimmed.to_string());
        }

        // Build errors (file:line:col format within test output)
        if trimmed.contains(".go:")
            && trimmed.contains(": ")
            && !trimmed.starts_with("---")
            && let Some(pkg_name) = extract_package_from_context(&current_package)
        {
            let pkg = packages.entry(pkg_name).or_default();
            if !pkg.build_errors.contains(&trimmed.to_string()) {
                pkg.build_errors.push(trimmed.to_string());
            }
        }
    }

    // Flush last test
    flush_test(
        &mut packages,
        &current_package,
        &current_test,
        &current_output,
    );

    build_go_test_summary(&packages)
}

fn flush_test(
    packages: &mut HashMap<String, PackageResult>,
    current_package: &str,
    current_test: &Option<String>,
    current_output: &[String],
) {
    if current_test.is_some()
        && !current_output.is_empty()
        && let Some(pkg_name) = extract_package_from_context(current_package)
    {
        let _pkg = packages.entry(pkg_name).or_default();
    }
}

fn extract_package_from_context(pkg: &str) -> Option<String> {
    if pkg.is_empty() {
        Some("(default)".to_string())
    } else {
        Some(pkg.to_string())
    }
}

fn build_go_test_summary(packages: &HashMap<String, PackageResult>) -> String {
    let total_pass: usize = packages.values().map(|p| p.pass).sum();
    let total_fail: usize = packages.values().map(|p| p.fail).sum();
    let total_skip: usize = packages.values().map(|p| p.skip).sum();
    let build_failures: usize = packages
        .values()
        .filter(|p| !p.build_errors.is_empty())
        .count();

    let has_failures = total_fail > 0 || build_failures > 0;

    if !has_failures && total_pass == 0 {
        return "Go test: No tests found".to_string();
    }

    if !has_failures {
        return format!(
            "Go test: {} passed in {} packages",
            total_pass,
            packages.len()
        );
    }

    let mut result = format!("Go test: {} passed, {} failed", total_pass, total_fail);
    if total_skip > 0 {
        result.push_str(&format!(", {} skipped", total_skip));
    }
    result.push_str(&format!(" in {} packages\n", packages.len()));

    // Show build errors first
    for (package, pkg_result) in packages.iter() {
        if pkg_result.build_errors.is_empty() {
            continue;
        }
        result.push_str(&format!(
            "\n{} [build errors]\n",
            compact_package_name(package)
        ));
        for err in pkg_result.build_errors.iter().take(10) {
            result.push_str(&format!("  {}\n", truncate(err, 120)));
        }
    }

    // Show failed tests
    for (package, pkg_result) in packages.iter() {
        if pkg_result.fail == 0 {
            continue;
        }
        result.push_str(&format!(
            "\n{} ({} passed, {} failed)\n",
            compact_package_name(package),
            pkg_result.pass,
            pkg_result.fail
        ));

        for (test, outputs) in &pkg_result.failed_tests {
            result.push_str(&format!("  FAIL {}\n", test));

            let relevant: Vec<&String> = outputs
                .iter()
                .filter(|line| {
                    let lower = line.to_lowercase();
                    !line.trim().is_empty()
                        && (lower.contains("error")
                            || lower.contains("expected")
                            || lower.contains("got")
                            || lower.contains("panic"))
                })
                .take(5)
                .collect();

            for line in relevant {
                result.push_str(&format!("     {}\n", truncate(line, 100)));
            }
        }
    }

    result.trim().to_string()
}

/// Filter go build output — show only errors.
fn filter_go_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();

        if trimmed.starts_with('#') && !lower.contains("error") {
            continue;
        }

        if !trimmed.is_empty()
            && (lower.contains("error")
                || trimmed.contains(".go:")
                || lower.contains("undefined")
                || lower.contains("cannot"))
        {
            errors.push(trimmed.to_string());
        }
    }

    if errors.is_empty() {
        return "Go build: ok".to_string();
    }

    let mut result = format!("Go build: {} errors\n", errors.len());
    for (i, error) in errors.iter().take(20).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 120)));
    }
    if errors.len() > 20 {
        result.push_str(&format!("\n... +{} more errors\n", errors.len() - 20));
    }

    result.trim().to_string()
}

/// Filter go vet output — show issues.
fn filter_go_vet(output: &str) -> String {
    let mut issues: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed.contains(".go:") {
            issues.push(trimmed.to_string());
        }
    }

    if issues.is_empty() {
        return "Go vet: ok".to_string();
    }

    let mut result = format!("Go vet: {} issues\n", issues.len());
    for (i, issue) in issues.iter().take(20).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(issue, 120)));
    }
    if issues.len() > 20 {
        result.push_str(&format!("\n... +{} more issues\n", issues.len() - 20));
    }

    result.trim().to_string()
}

fn compact_package_name(package: &str) -> String {
    if let Some(pos) = package.rfind('/') {
        package[pos + 1..].to_string()
    } else {
        package.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..s.floor_char_boundary(max.saturating_sub(3))])
    }
}

#[derive(Default)]
struct PackageResult {
    pass: usize,
    fail: usize,
    skip: usize,
    failed: bool,
    build_errors: Vec<String>,
    failed_tests: Vec<(String, Vec<String>)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_go_test_all_pass() {
        let output = "=== RUN   TestFoo\n--- PASS: TestFoo (0.00s)\n=== RUN   TestBar\n--- PASS: TestBar (0.00s)\nok\texample.com/pkg\t0.005s\n";
        let compressor = GoCompressor;
        let result = compressor.compress("go test ./...", output).unwrap();
        assert!(result.contains("2 passed"));
        assert!(!result.contains("failed"));
    }

    #[test]
    fn test_go_test_with_failures() {
        let output = "=== RUN   TestFoo\n--- PASS: TestFoo (0.00s)\n=== RUN   TestBar\n    bar_test.go:10: expected 5, got 3\n--- FAIL: TestBar (0.01s)\nFAIL\texample.com/pkg\t0.015s\n";
        let compressor = GoCompressor;
        let result = compressor.compress("go test ./...", output).unwrap();
        assert!(result.contains("1 passed, 1 failed"));
        assert!(result.contains("TestBar"));
    }

    #[test]
    fn test_go_build_success() {
        let result = filter_go_build("");
        assert!(result.contains("ok"));
    }

    #[test]
    fn test_go_build_errors() {
        let output = "# example.com/foo\nmain.go:10:5: undefined: missingFunc\nmain.go:15:2: cannot use x (type int) as type string\n";
        let compressor = GoCompressor;
        let result = compressor.compress("go build ./...", output).unwrap();
        assert!(result.contains("2 errors"));
        assert!(result.contains("undefined: missingFunc"));
    }

    #[test]
    fn test_go_vet_no_issues() {
        let result = filter_go_vet("");
        assert!(result.contains("ok"));
    }

    #[test]
    fn test_go_vet_with_issues() {
        let output = "main.go:42:2: Printf format %d has arg x of wrong type string\nutils.go:15:5: unreachable code\n";
        let compressor = GoCompressor;
        let result = compressor.compress("go vet ./...", output).unwrap();
        assert!(result.contains("2 issues"));
        assert!(result.contains("Printf format"));
    }

    #[test]
    fn test_unknown_subcommand() {
        let compressor = GoCompressor;
        assert!(compressor.compress("go run .", "Hello world\n").is_none());
    }

    #[test]
    fn test_compact_package_name() {
        assert_eq!(compact_package_name("github.com/user/repo/pkg"), "pkg");
        assert_eq!(compact_package_name("simple"), "simple");
    }
}
