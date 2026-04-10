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

//! Compressor for `curl` command output.
//!
//! Auto-detects JSON responses and shows schema (types instead of values).
//! Truncates long non-JSON output to a reasonable size.

use super::BashCompressor;

const MAX_LINES: usize = 30;
const MAX_LINE_LEN: usize = 200;

pub struct CurlCompressor;

impl BashCompressor for CurlCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return None;
        }

        // Only compress if there's enough output to benefit
        if trimmed.lines().count() < 10 && trimmed.len() < 500 {
            return None;
        }

        Some(filter_curl_output(trimmed))
    }
}

fn filter_curl_output(output: &str) -> String {
    // Try JSON detection: starts with { or [
    if (output.starts_with('{') || output.starts_with('['))
        && (output.ends_with('}') || output.ends_with(']'))
        && let Some(schema) = json_schema(output)
    {
        return schema;
    }

    // Not JSON: truncate long output
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() > MAX_LINES {
        let mut result: Vec<&str> = lines[..MAX_LINES].to_vec();
        result.push("");
        return format!(
            "{}\n... ({} more lines, {} bytes total)",
            result.join("\n"),
            lines.len() - MAX_LINES,
            output.len()
        );
    }

    // Short output: truncate long lines
    lines
        .iter()
        .map(|l| truncate(l, MAX_LINE_LEN))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Produce a JSON schema representation showing types instead of values.
fn json_schema(input: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let mut out = String::new();
    format_value(&value, &mut out, 0, 4);
    Some(out)
}

fn format_value(value: &serde_json::Value, out: &mut String, depth: usize, max_depth: usize) {
    let indent = "  ".repeat(depth);

    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(_) => out.push_str("bool"),
        serde_json::Value::Number(n) => {
            if n.is_f64() {
                out.push_str("float");
            } else {
                out.push_str("int");
            }
        }
        serde_json::Value::String(s) => {
            if s.len() > 50 {
                out.push_str(&format!("string({})", s.len()));
            } else {
                out.push_str("string");
            }
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                out.push_str("[]");
            } else if depth >= max_depth {
                out.push_str(&format!("[...{}]", arr.len()));
            } else {
                out.push_str(&format!("[{}] [\n", arr.len()));
                // Show schema of first element only
                out.push_str(&format!("{}  ", indent));
                format_value(&arr[0], out, depth + 1, max_depth);
                out.push('\n');
                out.push_str(&format!("{}]", indent));
            }
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
            } else if depth >= max_depth {
                out.push_str(&format!("{{...{}}}", map.len()));
            } else {
                out.push_str("{\n");
                for (i, (key, val)) in map.iter().enumerate() {
                    if i >= 20 {
                        out.push_str(&format!("{}  ... +{} more keys\n", indent, map.len() - 20));
                        break;
                    }
                    out.push_str(&format!("{}  \"{}\": ", indent, key));
                    format_value(val, out, depth + 1, max_depth);
                    out.push('\n');
                }
                out.push_str(&format!("{}}}", indent));
            }
        }
    }
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
    fn test_filter_curl_json() {
        let output = r#"{"name": "test", "count": 42, "items": [1, 2, 3]}"#;
        // Short JSON — won't compress (< 10 lines, < 500 bytes)
        let compressor = CurlCompressor;
        assert!(
            compressor
                .compress("curl https://api.example.com", output)
                .is_none()
        );
    }

    #[test]
    fn test_filter_curl_large_json() {
        let output = r#"{"name": "test", "count": 42, "items": [1, 2, 3], "description": "a very long description that makes this output larger than 500 bytes so the compressor kicks in and actually does something useful for us in this test case here we go adding more text to make it larger and larger", "extra1": "value1", "extra2": "value2", "extra3": "value3", "extra4": "value4", "extra5": "value5", "extra6": "value6", "extra7": "value7", "extra8": "value8"}"#;
        let result = filter_curl_output(output);
        assert!(result.contains("string"));
        assert!(result.contains("int"));
    }

    #[test]
    fn test_filter_curl_json_array() {
        let output = r#"[{"id": 1, "name": "a"}, {"id": 2, "name": "b"}, {"id": 3, "name": "c"}]"#;
        let result = filter_curl_output(output);
        assert!(result.contains("id"));
        assert!(result.contains("int"));
    }

    #[test]
    fn test_filter_curl_long_output() {
        let lines: Vec<String> = (0..50).map(|i| format!("Line {}", i)).collect();
        let output = lines.join("\n");
        let result = filter_curl_output(&output);
        assert!(result.contains("Line 0"));
        assert!(result.contains("Line 29"));
        assert!(result.contains("more lines"));
    }

    #[test]
    fn test_json_schema_basic() {
        let json = r#"{"name": "test", "count": 42, "active": true}"#;
        let result = json_schema(json).unwrap();
        assert!(result.contains("\"name\": string"));
        assert!(result.contains("\"count\": int"));
        assert!(result.contains("\"active\": bool"));
    }

    #[test]
    fn test_json_schema_nested() {
        let json = r#"{"user": {"name": "alice", "age": 30}}"#;
        let result = json_schema(json).unwrap();
        assert!(result.contains("\"user\":"));
        assert!(result.contains("\"name\": string"));
    }

    #[test]
    fn test_json_schema_array() {
        let json = r#"[{"id": 1}, {"id": 2}]"#;
        let result = json_schema(json).unwrap();
        assert!(result.contains("[2]"));
        assert!(result.contains("\"id\": int"));
    }

    #[test]
    fn test_compressor_skips_short_output() {
        let compressor = CurlCompressor;
        assert!(
            compressor
                .compress("curl https://api.example.com", "OK")
                .is_none()
        );
    }
}
