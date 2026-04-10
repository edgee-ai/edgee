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

//! Compressor for `psql` command output.
//!
//! Detects table and expanded display formats, strips borders/padding,
//! and produces compact tab-separated or key=value output.

use super::BashCompressor;

const MAX_TABLE_ROWS: usize = 30;
const MAX_EXPANDED_RECORDS: usize = 20;

pub struct PsqlCompressor;

impl BashCompressor for PsqlCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let result = filter_psql_output(output);
        if result == output {
            return None;
        }

        Some(result)
    }
}

fn filter_psql_output(output: &str) -> String {
    if is_expanded_format(output) {
        filter_expanded(output)
    } else if is_table_format(output) {
        filter_table(output)
    } else {
        // Passthrough: COPY results, notices, etc.
        output.to_string()
    }
}

fn is_table_format(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.trim().contains("-+-") || line.trim().contains("---+---"))
}

fn is_expanded_format(output: &str) -> bool {
    output.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("-[ RECORD ") && trimmed.contains("]-")
    })
}

fn is_separator_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '-' || c == '+')
}

fn is_row_count_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('(')
        && (trimmed.ends_with("rows)") || trimmed.ends_with("row)"))
        && trimmed[1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .count()
            > 0
}

/// Filter psql table format:
/// - Strip separator lines (----+----)
/// - Strip (N rows) footer
/// - Trim column padding
/// - Output tab-separated
fn filter_table(output: &str) -> String {
    let mut result = Vec::new();
    let mut data_rows = 0;
    let mut total_rows = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        if is_separator_line(trimmed) {
            continue;
        }

        if is_row_count_line(trimmed) {
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        // Data or header row with | delimiters
        if trimmed.contains('|') {
            total_rows += 1;
            // First row is header
            if total_rows > 1 {
                data_rows += 1;
            }

            if data_rows <= MAX_TABLE_ROWS || total_rows == 1 {
                let cols: Vec<&str> = trimmed.split('|').map(|c| c.trim()).collect();
                result.push(cols.join("\t"));
            }
        } else {
            // Non-table line (e.g., SET, NOTICE)
            result.push(trimmed.to_string());
        }
    }

    if data_rows > MAX_TABLE_ROWS {
        result.push(format!("... +{} more rows", data_rows - MAX_TABLE_ROWS));
    }

    result.join("\n")
}

/// Filter psql expanded format:
/// Convert -[ RECORD N ]- blocks to one-liner key=val format.
fn filter_expanded(output: &str) -> String {
    let mut result = Vec::new();
    let mut current_pairs: Vec<String> = Vec::new();
    let mut current_record: Option<String> = None;
    let mut record_count = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        if is_row_count_line(trimmed) {
            continue;
        }

        if let Some(record_num) = parse_record_header(trimmed) {
            // Flush previous record
            if let Some(rec) = current_record.take() {
                if record_count <= MAX_EXPANDED_RECORDS {
                    result.push(format!("{} {}", rec, current_pairs.join(" ")));
                }
                current_pairs.clear();
            }
            record_count += 1;
            current_record = Some(format!("[{}]", record_num));
        } else if trimmed.contains('|') && current_record.is_some() {
            let parts: Vec<&str> = trimmed.splitn(2, '|').collect();
            if parts.len() == 2 {
                let key = parts[0].trim();
                let val = parts[1].trim();
                current_pairs.push(format!("{}={}", key, val));
            }
        } else if trimmed.is_empty() {
            continue;
        } else if current_record.is_none() {
            result.push(trimmed.to_string());
        }
    }

    // Flush last record
    if let Some(rec) = current_record.take()
        && record_count <= MAX_EXPANDED_RECORDS
    {
        result.push(format!("{} {}", rec, current_pairs.join(" ")));
    }

    if record_count > MAX_EXPANDED_RECORDS {
        result.push(format!(
            "... +{} more records",
            record_count - MAX_EXPANDED_RECORDS
        ));
    }

    result.join("\n")
}

/// Parse a record header line like "-[ RECORD 1 ]----" and return the record number.
fn parse_record_header(line: &str) -> Option<&str> {
    let line = line.trim();
    if !line.starts_with("-[ RECORD ") {
        return None;
    }
    let after = &line["-[ RECORD ".len()..];
    let end = after.find(' ').or_else(|| after.find(']'))?;
    let num = &after[..end];
    if num.chars().all(|c| c.is_ascii_digit()) {
        Some(num)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_table_basic() {
        let input = " id | name  | email\n----+-------+---------\n  1 | alice | a@b.com\n  2 | bob   | b@b.com\n(2 rows)\n";
        let result = filter_table(input);
        assert!(result.contains("id\tname\temail"));
        assert!(result.contains("1\talice\ta@b.com"));
        assert!(result.contains("2\tbob\tb@b.com"));
        assert!(!result.contains("----"));
        assert!(!result.contains("(2 rows)"));
    }

    #[test]
    fn test_filter_expanded_basic() {
        let input =
            "-[ RECORD 1 ]----\nid   | 1\nname | alice\n-[ RECORD 2 ]----\nid   | 2\nname | bob\n";
        let result = filter_expanded(input);
        assert!(result.contains("[1] id=1 name=alice"));
        assert!(result.contains("[2] id=2 name=bob"));
    }

    #[test]
    fn test_is_table_format() {
        assert!(is_table_format(
            " id | name\n----+------\n  1 | foo\n(1 row)\n"
        ));
        assert!(!is_table_format("COPY 5\n"));
        assert!(!is_table_format("SET\n"));
    }

    #[test]
    fn test_is_expanded_format() {
        assert!(is_expanded_format(
            "-[ RECORD 1 ]----\nid | 1\nname | foo\n"
        ));
        assert!(!is_expanded_format(" id | name\n----+------\n  1 | foo\n"));
    }

    #[test]
    fn test_filter_table_overflow() {
        let mut lines = vec![" id | val".to_string(), "----+-----".to_string()];
        for i in 1..=40 {
            lines.push(format!("  {} | row{}", i, i));
        }
        lines.push("(40 rows)".to_string());
        let input = lines.join("\n");

        let result = filter_table(&input);
        assert!(result.contains("... +10 more rows"));
    }

    #[test]
    fn test_filter_expanded_overflow() {
        let mut lines = Vec::new();
        for i in 1..=25 {
            lines.push(format!("-[ RECORD {} ]----", i));
            lines.push(format!("id   | {}", i));
            lines.push(format!("name | user{}", i));
        }
        let input = lines.join("\n");

        let result = filter_expanded(&input);
        assert!(result.contains("... +5 more records"));
    }

    #[test]
    fn test_filter_psql_passthrough() {
        let input = "COPY 5\n";
        let result = filter_psql_output(input);
        assert_eq!(result, "COPY 5\n");
    }

    #[test]
    fn test_filter_psql_routes_to_table() {
        let input = " id | name\n----+------\n  1 | foo\n(1 row)\n";
        let result = filter_psql_output(input);
        assert!(result.contains("id\tname"));
        assert!(!result.contains("----"));
    }

    #[test]
    fn test_filter_psql_routes_to_expanded() {
        let input = "-[ RECORD 1 ]----\nid | 1\nname | foo\n";
        let result = filter_psql_output(input);
        assert!(result.contains("[1]"));
        assert!(result.contains("id=1"));
    }

    #[test]
    fn test_parse_record_header() {
        assert_eq!(parse_record_header("-[ RECORD 1 ]----"), Some("1"));
        assert_eq!(parse_record_header("-[ RECORD 42 ]------"), Some("42"));
        assert!(parse_record_header("not a record").is_none());
        assert!(parse_record_header("----+----").is_none());
    }

    #[test]
    fn test_filter_table_strips_row_count() {
        let input = " c\n---\n 1\n(1 row)\n";
        let result = filter_table(input);
        assert!(!result.contains("(1 row)"));
    }

    #[test]
    fn test_filter_expanded_strips_row_count() {
        let input = "-[ RECORD 1 ]----\nid | 1\n(1 row)\n";
        let result = filter_expanded(input);
        assert!(!result.contains("(1 row)"));
    }

    #[test]
    fn test_compressor_returns_none_for_empty() {
        let compressor = PsqlCompressor;
        assert!(compressor.compress("psql", "").is_none());
    }
}
