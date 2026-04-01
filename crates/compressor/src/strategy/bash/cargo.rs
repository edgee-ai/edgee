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

//! Compressor for `cargo` command output.
//!
//! Strips Compiling/Downloading/Checking noise lines and keeps only
//! errors, warnings, and summary for build/test/clippy/check output.

use std::collections::HashMap;

use super::BashCompressor;

pub struct CargoCompressor;

impl BashCompressor for CargoCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        let subcommand = parse_cargo_subcommand(command);
        match subcommand {
            "build" | "check" | "b" => Some(filter_cargo_build(output)),
            "test" | "t" => Some(filter_cargo_test(output)),
            "clippy" => Some(filter_cargo_clippy(output)),
            _ => None, // Don't compress unknown subcommands
        }
    }
}

fn parse_cargo_subcommand(command: &str) -> &str {
    for arg in command.split_whitespace().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        return arg;
    }
    ""
}

fn is_noise_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("Compiling")
        || trimmed.starts_with("Checking")
        || trimmed.starts_with("Downloading")
        || trimmed.starts_with("Downloaded")
        || trimmed.starts_with("Finished")
        || trimmed.starts_with("Locking")
        || trimmed.starts_with("Updating")
        || trimmed.starts_with("Blocking waiting for file lock")
}

/// Filter cargo build/check output: strip compilation lines, keep errors + summary.
fn filter_cargo_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings = 0;
    let mut error_count = 0;
    let mut compiled = 0;
    let mut in_error = false;
    let mut current_error: Vec<String> = Vec::new();

    for line in output.lines() {
        if is_noise_line(line) {
            compiled += 1;
            continue;
        }

        if line.starts_with("error[") || line.starts_with("error:") {
            if line.contains("aborting due to") || line.contains("could not compile") {
                continue;
            }
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            error_count += 1;
            in_error = true;
            current_error.push(line.to_string());
        } else if line.starts_with("warning:")
            && line.contains("generated")
            && line.contains("warning")
        {
            continue; // Skip summary warning lines
        } else if line.starts_with("warning:") || line.starts_with("warning[") {
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            warnings += 1;
            in_error = true;
            current_error.push(line.to_string());
        } else if in_error {
            if line.trim().is_empty() && current_error.len() > 3 {
                errors.push(current_error.join("\n"));
                current_error.clear();
                in_error = false;
            } else {
                current_error.push(line.to_string());
            }
        }
    }

    if !current_error.is_empty() {
        errors.push(current_error.join("\n"));
    }

    if error_count == 0 && warnings == 0 {
        return format!("ok ({} crates compiled)\n", compiled);
    }

    let mut result = format!(
        "cargo build: {} errors, {} warnings ({} crates)\n",
        error_count, warnings, compiled
    );

    for err in errors.iter().take(15) {
        result.push_str(err);
        result.push('\n');
        result.push('\n');
    }

    if errors.len() > 15 {
        result.push_str(&format!("... +{} more issues\n", errors.len() - 15));
    }

    result
}

/// Filter cargo test output: show only failures + summary.
fn filter_cargo_test(output: &str) -> String {
    let mut failures: Vec<String> = Vec::new();
    let mut summary_lines: Vec<String> = Vec::new();
    let mut in_failure_section = false;
    let mut current_failure: Vec<String> = Vec::new();

    for line in output.lines() {
        if is_noise_line(line) {
            continue;
        }

        // Skip "running N tests" and individual "test ... ok" lines
        if line.starts_with("running ") || (line.starts_with("test ") && line.ends_with("... ok")) {
            continue;
        }

        if line == "failures:" {
            in_failure_section = true;
            continue;
        }

        if in_failure_section {
            if line.starts_with("test result:") {
                in_failure_section = false;
                summary_lines.push(line.to_string());
            } else if line.starts_with("    ") || line.starts_with("---- ") {
                current_failure.push(line.to_string());
            } else if line.trim().is_empty() && !current_failure.is_empty() {
                failures.push(current_failure.join("\n"));
                current_failure.clear();
            } else if !line.trim().is_empty() {
                current_failure.push(line.to_string());
            }
        }

        if !in_failure_section && line.starts_with("test result:") {
            summary_lines.push(line.to_string());
        }
    }

    if !current_failure.is_empty() {
        failures.push(current_failure.join("\n"));
    }

    if failures.is_empty() && !summary_lines.is_empty() {
        let mut result = String::new();
        for line in &summary_lines {
            result.push_str(&format!("ok {}\n", line));
        }
        return result;
    }

    let mut result = String::new();

    if !failures.is_empty() {
        result.push_str(&format!("FAILURES ({}):\n\n", failures.len()));
        for (i, failure) in failures.iter().enumerate().take(10) {
            let truncated = if failure.len() > 200 {
                format!("{}...", &failure[..197])
            } else {
                failure.clone()
            };
            result.push_str(&format!("{}. {}\n\n", i + 1, truncated));
        }
        if failures.len() > 10 {
            result.push_str(&format!("... +{} more failures\n", failures.len() - 10));
        }
    }

    for line in &summary_lines {
        result.push_str(line);
        result.push('\n');
    }

    if result.trim().is_empty() {
        // Fallback: return last few meaningful lines
        let meaningful: Vec<&str> = output
            .lines()
            .filter(|l| !l.trim().is_empty() && !is_noise_line(l))
            .collect();
        for line in meaningful.iter().rev().take(5).rev() {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

/// Filter cargo clippy output: group warnings by lint rule.
fn filter_cargo_clippy(output: &str) -> String {
    let mut by_rule: HashMap<String, Vec<String>> = HashMap::new();
    let mut error_count = 0;
    let mut warning_count = 0;
    let mut current_rule = String::new();

    for line in output.lines() {
        if is_noise_line(line) {
            continue;
        }

        if (line.starts_with("warning:") || line.starts_with("warning["))
            || (line.starts_with("error:") || line.starts_with("error["))
        {
            if line.contains("generated") && line.contains("warning") {
                continue;
            }
            if line.contains("aborting due to") || line.contains("could not compile") {
                continue;
            }

            let is_error = line.starts_with("error");
            if is_error {
                error_count += 1;
            } else {
                warning_count += 1;
            }

            current_rule = if let Some(bracket_start) = line.rfind('[') {
                if let Some(bracket_end) = line.rfind(']') {
                    line[bracket_start + 1..bracket_end].to_string()
                } else {
                    line.to_string()
                }
            } else {
                let prefix = if is_error { "error: " } else { "warning: " };
                line.strip_prefix(prefix).unwrap_or(line).to_string()
            };
        } else if line.trim_start().starts_with("--> ") {
            let location = line.trim_start().trim_start_matches("--> ").to_string();
            if !current_rule.is_empty() {
                by_rule
                    .entry(current_rule.clone())
                    .or_default()
                    .push(location);
            }
        }
    }

    if error_count == 0 && warning_count == 0 {
        return "ok clippy\n".to_string();
    }

    let mut result = format!(
        "cargo clippy: {} errors, {} warnings\n\n",
        error_count, warning_count
    );

    let mut rule_counts: Vec<_> = by_rule.iter().collect();
    rule_counts.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (rule, locations) in rule_counts.iter().take(15) {
        result.push_str(&format!("  {} ({}x)\n", rule, locations.len()));
        for loc in locations.iter().take(3) {
            result.push_str(&format!("    {}\n", loc));
        }
        if locations.len() > 3 {
            result.push_str(&format!("    ... +{} more\n", locations.len() - 3));
        }
    }

    if by_rule.len() > 15 {
        result.push_str(&format!("\n... +{} more rules\n", by_rule.len() - 15));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cargo_subcommand() {
        assert_eq!(parse_cargo_subcommand("cargo build"), "build");
        assert_eq!(parse_cargo_subcommand("cargo test --release"), "test");
        assert_eq!(parse_cargo_subcommand("cargo clippy -- -W"), "clippy");
        assert_eq!(parse_cargo_subcommand("cargo"), "");
    }

    #[test]
    fn test_build_success() {
        let output = "   Compiling libc v0.2.153\n   Compiling myapp v0.1.0\n    Finished dev [unoptimized + debuginfo] target(s) in 5.2s\n";
        let compressor = CargoCompressor;
        let result = compressor.compress("cargo build", output).unwrap();
        assert!(result.contains("ok"));
        assert!(result.contains("3 crates compiled"));
        assert!(!result.contains("Compiling"));
    }

    #[test]
    fn test_build_errors() {
        let output = "   Compiling myapp v0.1.0\nerror[E0308]: mismatched types\n --> src/main.rs:10:5\n  |\n10|     \"hello\"\n  |     ^^^^^^^ expected `i32`, found `&str`\n\nerror: aborting due to 1 previous error\n";
        let compressor = CargoCompressor;
        let result = compressor.compress("cargo build", output).unwrap();
        assert!(result.contains("1 errors"));
        assert!(result.contains("E0308"));
        assert!(!result.contains("Compiling"));
        assert!(!result.contains("aborting"));
    }

    #[test]
    fn test_test_all_pass() {
        let output = "   Compiling myapp v0.1.0\n    Finished test target(s) in 2.5s\nrunning 15 tests\ntest foo::test_a ... ok\ntest foo::test_b ... ok\n\ntest result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
        let compressor = CargoCompressor;
        let result = compressor.compress("cargo test", output).unwrap();
        assert!(result.contains("ok test result:"));
        assert!(result.contains("15 passed"));
        assert!(!result.contains("Compiling"));
        assert!(!result.contains("test foo::test_a"));
    }

    #[test]
    fn test_test_failures() {
        let output = "running 2 tests\ntest foo::test_a ... ok\ntest foo::test_b ... FAILED\n\nfailures:\n\n---- foo::test_b stdout ----\nthread 'foo::test_b' panicked at 'assert_eq!(1, 2)'\n\nfailures:\n    foo::test_b\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n";
        let compressor = CargoCompressor;
        let result = compressor.compress("cargo test", output).unwrap();
        assert!(result.contains("FAILURES"));
        assert!(result.contains("test_b"));
    }

    #[test]
    fn test_clippy_clean() {
        let output = "    Checking myapp v0.1.0\n    Finished dev target(s) in 1.5s\n";
        let compressor = CargoCompressor;
        let result = compressor.compress("cargo clippy", output).unwrap();
        assert!(result.contains("ok clippy"));
    }

    #[test]
    fn test_clippy_warnings() {
        let output = "    Checking myapp v0.1.0\nwarning: unused variable: `x` [unused_variables]\n --> src/main.rs:10:9\n\nwarning: `myapp` (bin) generated 1 warning\n    Finished dev target(s) in 1.5s\n";
        let compressor = CargoCompressor;
        let result = compressor.compress("cargo clippy", output).unwrap();
        assert!(result.contains("0 errors, 1 warnings"));
        assert!(result.contains("unused_variables"));
    }

    #[test]
    fn test_unknown_subcommand_returns_none() {
        let compressor = CargoCompressor;
        assert!(compressor.compress("cargo run", "Hello world\n").is_none());
    }

    #[test]
    fn test_check_uses_build_filter() {
        let output = "    Checking myapp v0.1.0\n    Finished dev target(s)\n";
        let compressor = CargoCompressor;
        let result = compressor.compress("cargo check", output).unwrap();
        assert!(result.contains("ok"));
    }
}
