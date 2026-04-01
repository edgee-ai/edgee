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

//! Compressor for `eslint` command output.
//!
//! Groups ESLint issues by rule and file, providing a compact summary.
//! Handles both the default formatter and JSON output.

use std::collections::HashMap;

use super::BashCompressor;

pub struct EslintCompressor;

impl BashCompressor for EslintCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        // Try JSON format first (if user ran eslint -f json)
        if output.trim().starts_with('[') {
            return filter_eslint_json(output);
        }

        // Default text format
        filter_eslint_text(output)
    }
}

/// Filter ESLint JSON output — group by rule and file.
fn filter_eslint_json(output: &str) -> Option<String> {
    let results: Vec<EslintJsonResult> = match serde_json::from_str(output) {
        Ok(r) => r,
        Err(_) => return None,
    };

    let total_errors: usize = results.iter().map(|r| r.error_count).sum();
    let total_warnings: usize = results.iter().map(|r| r.warning_count).sum();
    let total_files = results.iter().filter(|r| !r.messages.is_empty()).count();

    if total_errors == 0 && total_warnings == 0 {
        return Some("ESLint: No issues found".to_string());
    }

    // Group by rule
    let mut by_rule: HashMap<String, usize> = HashMap::new();
    for result in &results {
        for msg in &result.messages {
            let rule = msg.rule_id.as_deref().unwrap_or("unknown");
            *by_rule.entry(rule.to_string()).or_insert(0) += 1;
        }
    }

    let mut out = format!(
        "ESLint: {} errors, {} warnings in {} files\n\n",
        total_errors, total_warnings, total_files
    );

    // Top rules
    let mut rule_counts: Vec<_> = by_rule.iter().collect();
    rule_counts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    if !rule_counts.is_empty() {
        for (rule, count) in rule_counts.iter().take(10) {
            out.push_str(&format!("  {} ({}x)\n", rule, count));
        }
        if rule_counts.len() > 10 {
            out.push_str(&format!("  ... +{} more rules\n", rule_counts.len() - 10));
        }
    }

    Some(out.trim().to_string())
}

/// Filter ESLint default text output — group issues by file with counts.
fn filter_eslint_text(output: &str) -> Option<String> {
    let mut files: Vec<FileIssues> = Vec::new();
    let mut current_file = String::new();
    let mut current_issues: Vec<Issue> = Vec::new();
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;
    let mut by_rule: HashMap<String, usize> = HashMap::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines and summary lines
        if trimmed.is_empty() {
            continue;
        }

        // Summary line at the end
        if trimmed.starts_with('\u{2716}') || trimmed.starts_with("✖") {
            continue;
        }

        // File path line (not indented, contains / or \)
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && (trimmed.contains('/') || trimmed.contains('\\'))
            && !trimmed.contains("  ")
        {
            // Flush previous file
            if !current_file.is_empty() && !current_issues.is_empty() {
                files.push(FileIssues {
                    path: current_file.clone(),
                    issues: std::mem::take(&mut current_issues),
                });
            }
            current_file = trimmed.to_string();
            continue;
        }

        // Issue line: "  line:col  severity  message  rule"
        if (line.starts_with(' ') || line.starts_with('\t')) && !current_file.is_empty() {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let severity = if parts.contains(&"error") {
                    total_errors += 1;
                    "error"
                } else if parts.contains(&"warning") {
                    total_warnings += 1;
                    "warning"
                } else {
                    continue;
                };

                // Last part is usually the rule name
                let rule = parts.last().unwrap_or(&"");
                *by_rule.entry(rule.to_string()).or_insert(0) += 1;

                current_issues.push(Issue {
                    severity: severity.to_string(),
                    _rule: rule.to_string(),
                    _location: parts.first().unwrap_or(&"").to_string(),
                });
            }
        }
    }

    // Flush last file
    if !current_file.is_empty() && !current_issues.is_empty() {
        files.push(FileIssues {
            path: current_file,
            issues: current_issues,
        });
    }

    if files.is_empty() {
        return None;
    }

    let mut result = format!(
        "ESLint: {} errors, {} warnings in {} files\n\n",
        total_errors,
        total_warnings,
        files.len()
    );

    // Top rules
    let mut rule_counts: Vec<_> = by_rule.iter().collect();
    rule_counts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    if !rule_counts.is_empty() {
        result.push_str("Top rules:\n");
        for (rule, count) in rule_counts.iter().take(10) {
            result.push_str(&format!("  {} ({}x)\n", rule, count));
        }
        result.push('\n');
    }

    // Top files
    let mut sorted_files = files;
    sorted_files.sort_by(|a, b| b.issues.len().cmp(&a.issues.len()));

    result.push_str("Files:\n");
    for file in sorted_files.iter().take(15) {
        let short_path = compact_path(&file.path);
        let errors = file.issues.iter().filter(|i| i.severity == "error").count();
        let warnings = file
            .issues
            .iter()
            .filter(|i| i.severity == "warning")
            .count();
        result.push_str(&format!(
            "  {} ({} errors, {} warnings)\n",
            short_path, errors, warnings
        ));
    }

    if sorted_files.len() > 15 {
        result.push_str(&format!("  ... +{} more files\n", sorted_files.len() - 15));
    }

    Some(result.trim().to_string())
}

fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    if let Some(pos) = path.rfind("/src/") {
        format!("src/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/lib/") {
        format!("lib/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

struct FileIssues {
    path: String,
    issues: Vec<Issue>,
}

struct Issue {
    severity: String,
    _rule: String,
    _location: String,
}

#[derive(serde::Deserialize)]
struct EslintJsonResult {
    #[serde(rename = "filePath")]
    _file_path: String,
    messages: Vec<EslintJsonMessage>,
    #[serde(rename = "errorCount")]
    error_count: usize,
    #[serde(rename = "warningCount")]
    warning_count: usize,
}

#[derive(serde::Deserialize)]
struct EslintJsonMessage {
    #[serde(rename = "ruleId")]
    rule_id: Option<String>,
    #[serde(rename = "severity")]
    _severity: u8,
    #[serde(rename = "message")]
    _message: String,
    #[serde(rename = "line")]
    _line: usize,
    #[serde(rename = "column")]
    _column: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_eslint_json() {
        let json = r#"[
            {
                "filePath": "/Users/test/project/src/utils.ts",
                "messages": [
                    {"ruleId": "prefer-const", "severity": 1, "message": "Use const", "line": 10, "column": 5},
                    {"ruleId": "prefer-const", "severity": 1, "message": "Use const", "line": 15, "column": 5}
                ],
                "errorCount": 0,
                "warningCount": 2
            },
            {
                "filePath": "/Users/test/project/src/api.ts",
                "messages": [
                    {"ruleId": "@typescript-eslint/no-unused-vars", "severity": 2, "message": "Variable x is unused", "line": 20, "column": 10}
                ],
                "errorCount": 1,
                "warningCount": 0
            }
        ]"#;

        let compressor = EslintCompressor;
        let result = compressor.compress("eslint -f json .", json).unwrap();
        assert!(result.contains("ESLint:"));
        assert!(result.contains("prefer-const"));
        assert!(result.contains("no-unused-vars"));
    }

    #[test]
    fn test_filter_eslint_text() {
        let output = "/Users/test/src/utils.ts\n  10:5  warning  Use const instead of let  prefer-const\n  15:5  warning  Use const instead of let  prefer-const\n\n/Users/test/src/api.ts\n  20:10  error  Variable x is unused  @typescript-eslint/no-unused-vars\n\n✖ 3 problems (1 error, 2 warnings)\n";
        let compressor = EslintCompressor;
        let result = compressor.compress("eslint .", output).unwrap();
        assert!(result.contains("ESLint: 1 errors, 2 warnings"));
        assert!(result.contains("prefer-const"));
    }

    #[test]
    fn test_filter_eslint_json_no_issues() {
        let json =
            r#"[{"filePath": "/test.ts", "messages": [], "errorCount": 0, "warningCount": 0}]"#;
        let result = filter_eslint_json(json).unwrap();
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/src/utils.ts"),
            "src/utils.ts"
        );
        assert_eq!(compact_path("simple.ts"), "simple.ts");
    }

    #[test]
    fn test_compressor_returns_none_for_empty() {
        let compressor = EslintCompressor;
        assert!(compressor.compress("eslint .", "").is_none());
    }
}
