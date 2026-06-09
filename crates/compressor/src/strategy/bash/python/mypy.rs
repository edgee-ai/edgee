//! Compressor for `mypy` (Python type checker) output.
//!
//! Groups errors by file, strips `note:` lines (verbose URL hints, etc.),
//! and preserves the "Found N errors in M files" summary.

use super::BashCompressor;

pub struct MypyCompressor;

impl BashCompressor for MypyCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let compressed = compress_mypy_output(output);
        if compressed.trim() == output.trim() {
            return None;
        }

        Some(compressed)
    }
}

struct MypyDiagnostic<'a> {
    path: &'a str,
    location: &'a str, // "line" or "line:col"
    severity: &'a str, // "error" or "note" or "warning"
    message: &'a str,
}

/// Parse a mypy diagnostic line.
///
/// mypy emits: `path:line: error: message [code]`
///         or: `path:line:col: error: message [code]`
fn parse_mypy_line(line: &str) -> Option<MypyDiagnostic<'_>> {
    // Look for ": error: ", ": note: ", or ": warning: "
    for severity in &["error", "note", "warning"] {
        let needle = format!(": {severity}: ");
        if let Some(sev_pos) = line.find(needle.as_str()) {
            let loc_part = &line[..sev_pos]; // "path:line" or "path:line:col"
            let message = &line[sev_pos + needle.len()..];

            // Extract path: everything up to the first colon followed by a digit
            let path = extract_file_path(loc_part)?;
            let location = &loc_part[path.len() + 1..]; // skip the colon

            return Some(MypyDiagnostic {
                path,
                location,
                severity,
                message,
            });
        }
    }
    None
}

/// Find where the file path ends in a `path:line` or `path:line:col` string.
fn extract_file_path(loc: &str) -> Option<&str> {
    let bytes = loc.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            return Some(&loc[..i]);
        }
    }
    None
}

fn compress_mypy_output(output: &str) -> String {
    let mut file_order: Vec<String> = Vec::new();
    let mut file_errors: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut summary_lines: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Summary / result lines
        if trimmed.starts_with("Found ")
            || trimmed.starts_with("Success:")
            || trimmed.starts_with("error: ")
        {
            summary_lines.push(trimmed.to_string());
            continue;
        }

        if let Some(d) = parse_mypy_line(trimmed) {
            // Skip note lines — they're supplementary context and often just URLs/hints
            if d.severity == "note" {
                continue;
            }

            if !file_errors.contains_key(d.path) {
                file_order.push(d.path.to_string());
                file_errors.insert(d.path.to_string(), Vec::new());
            }
            file_errors
                .get_mut(d.path)
                .unwrap()
                .push(format!("  {}: {}: {}", d.location, d.severity, d.message));
        }
    }

    if file_order.is_empty() && summary_lines.is_empty() {
        return output.trim().to_string();
    }

    let mut result = String::new();
    for path in &file_order {
        let errors = &file_errors[path];
        let n = errors.len();
        result.push_str(&format!(
            "{path} ({n} {}):\n",
            if n == 1 { "error" } else { "errors" }
        ));
        for e in errors {
            result.push_str(e);
            result.push('\n');
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

    const MYPY_OUTPUT: &str =
        "src/main.py:10: error: Argument 1 to \"foo\" has incompatible type \"str\"; expected \"int\"  [arg-type]
src/main.py:15: note: Revealed type is \"builtins.str\"
src/utils.py:5: error: Cannot find implementation or library stub for module named \"requests\"  [import]
src/utils.py:5: note: See https://mypy.readthedocs.io/en/stable/running_mypy.html#missing-imports
Found 2 errors in 2 files (checked 5 source files)
";

    #[test]
    fn test_groups_by_file() {
        let result = MypyCompressor.compress("mypy src/", MYPY_OUTPUT).unwrap();
        assert!(result.contains("src/main.py (1 error):"));
        assert!(result.contains("src/utils.py (1 error):"));
    }

    #[test]
    fn test_strips_note_lines() {
        let result = MypyCompressor.compress("mypy src/", MYPY_OUTPUT).unwrap();
        assert!(!result.contains("Revealed type"));
        assert!(!result.contains("mypy.readthedocs.io"));
    }

    #[test]
    fn test_keeps_errors() {
        let result = MypyCompressor.compress("mypy src/", MYPY_OUTPUT).unwrap();
        assert!(result.contains("incompatible type"));
        assert!(result.contains("Cannot find implementation"));
    }

    #[test]
    fn test_keeps_summary() {
        let result = MypyCompressor.compress("mypy src/", MYPY_OUTPUT).unwrap();
        assert!(result.contains("Found 2 errors in 2 files"));
    }

    #[test]
    fn test_success_output() {
        let success = "Success: no issues found in 5 source files\n";
        // Already one line, may be None or Some
        let result = MypyCompressor.compress("mypy src/", success);
        if let Some(r) = result {
            assert!(r.contains("Success"));
        }
    }

    #[test]
    fn test_empty_passthrough() {
        assert!(MypyCompressor.compress("mypy src/", "").is_none());
    }

    #[test]
    fn test_extract_file_path() {
        assert_eq!(extract_file_path("src/main.py:10"), Some("src/main.py"));
        assert_eq!(extract_file_path("src/main.py:10:5"), Some("src/main.py"));
        assert_eq!(extract_file_path("no_colon_here"), None);
    }

    #[test]
    fn test_parse_error_line() {
        let d = parse_mypy_line("src/main.py:10: error: Incompatible type [arg-type]").unwrap();
        assert_eq!(d.path, "src/main.py");
        assert_eq!(d.location, "10");
        assert_eq!(d.severity, "error");
        assert!(d.message.contains("Incompatible type"));
    }

    #[test]
    fn test_parse_note_line() {
        let d = parse_mypy_line("src/main.py:10: note: See https://example.com").unwrap();
        assert_eq!(d.severity, "note");
    }

    #[test]
    fn test_two_errors_same_file() {
        let output = "src/foo.py:1: error: first error  [code1]
src/foo.py:2: error: second error  [code2]
Found 2 errors in 1 file (checked 3 source files)
";
        let result = MypyCompressor.compress("mypy .", output).unwrap();
        assert!(result.contains("src/foo.py (2 errors):"));
    }
}
