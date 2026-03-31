//! Compressor for `tsc` (TypeScript compiler) output.
//!
//! Groups TypeScript errors by file, shows error codes and messages,
//! and provides a summary with top error codes.

use std::collections::HashMap;

use super::BashCompressor;

pub struct TscCompressor;

impl BashCompressor for TscCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        Some(filter_tsc_output(output))
    }
}

struct TsError {
    file: String,
    line: usize,
    code: String,
    message: String,
    context_lines: Vec<String>,
}

/// Filter TypeScript compiler output — group errors by file.
fn filter_tsc_output(output: &str) -> String {
    let mut errors: Vec<TsError> = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if let Some(err) = parse_tsc_error(line) {
            let mut ts_err = err;

            // Capture continuation lines (indented context from tsc)
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                if !next.is_empty()
                    && (next.starts_with("  ") || next.starts_with('\t'))
                    && parse_tsc_error(next).is_none()
                {
                    ts_err.context_lines.push(next.trim().to_string());
                    i += 1;
                } else {
                    break;
                }
            }

            errors.push(ts_err);
        } else {
            i += 1;
        }
    }

    if errors.is_empty() {
        if output.contains("Found 0 errors") {
            return "TypeScript: No errors found".to_string();
        }
        return output.to_string();
    }

    // Group by file
    let mut by_file: HashMap<String, Vec<&TsError>> = HashMap::new();
    for err in &errors {
        by_file.entry(err.file.clone()).or_default().push(err);
    }

    // Count by error code
    let mut by_code: HashMap<String, usize> = HashMap::new();
    for err in &errors {
        *by_code.entry(err.code.clone()).or_insert(0) += 1;
    }

    let mut result = format!(
        "TypeScript: {} errors in {} files\n",
        errors.len(),
        by_file.len()
    );

    // Top error codes summary
    let mut code_counts: Vec<_> = by_code.iter().collect();
    code_counts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    if code_counts.len() > 1 {
        let codes_str: Vec<String> = code_counts
            .iter()
            .take(5)
            .map(|(code, count)| format!("{} ({}x)", code, count))
            .collect();
        result.push_str(&format!("Top codes: {}\n\n", codes_str.join(", ")));
    }

    // Files sorted by error count
    let mut files_sorted: Vec<_> = by_file.iter().collect();
    files_sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (file, file_errors) in &files_sorted {
        result.push_str(&format!("{} ({} errors)\n", file, file_errors.len()));

        for err in *file_errors {
            result.push_str(&format!(
                "  L{}: {} {}\n",
                err.line,
                err.code,
                truncate(&err.message, 120)
            ));
            for ctx in &err.context_lines {
                result.push_str(&format!("    {}\n", truncate(ctx, 120)));
            }
        }
        result.push('\n');
    }

    result.trim().to_string()
}

/// Parse a tsc error line like: src/file.ts(12,5): error TS2322: Type 'string' is not assignable.
fn parse_tsc_error(line: &str) -> Option<TsError> {
    // Find the pattern: file(line,col): error TSxxxx: message
    let paren_start = line.find('(')?;
    let paren_end = line[paren_start..].find(')')? + paren_start;

    let file = &line[..paren_start];
    let coords = &line[paren_start + 1..paren_end];

    let after_paren = &line[paren_end + 1..];
    if !after_paren.contains("error TS") && !after_paren.contains("warning TS") {
        return None;
    }

    let line_num: usize = coords.split(',').next()?.parse().ok()?;

    // Extract TS code
    let ts_start = after_paren.find("TS")?;
    let code_start = ts_start;
    let code_end = after_paren[code_start..]
        .find(':')
        .map(|i| i + code_start)
        .unwrap_or(after_paren.len());
    let code = after_paren[code_start..code_end].trim().to_string();

    let message = if code_end < after_paren.len() {
        after_paren[code_end + 1..].trim().to_string()
    } else {
        String::new()
    };

    Some(TsError {
        file: file.trim().to_string(),
        line: line_num,
        code,
        message,
        context_lines: Vec::new(),
    })
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
    fn test_filter_tsc_output() {
        let output = "src/server/api/auth.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.\nsrc/server/api/auth.ts(15,10): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.\nsrc/components/Button.tsx(8,3): error TS2339: Property 'onClick' does not exist on type 'ButtonProps'.\nsrc/components/Button.tsx(10,5): error TS2322: Type 'string' is not assignable to type 'number'.\n\nFound 4 errors in 2 files.\n";
        let result = filter_tsc_output(output);
        assert!(result.contains("TypeScript: 4 errors in 2 files"));
        assert!(result.contains("auth.ts (2 errors)"));
        assert!(result.contains("Button.tsx (2 errors)"));
        assert!(result.contains("TS2322"));
    }

    #[test]
    fn test_every_error_message_shown() {
        let output = "src/api.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.\nsrc/api.ts(20,5): error TS2322: Type 'boolean' is not assignable to type 'string'.\nsrc/api.ts(30,5): error TS2322: Type 'null' is not assignable to type 'object'.\n";
        let result = filter_tsc_output(output);
        assert!(result.contains("Type 'string' is not assignable to type 'number'"));
        assert!(result.contains("Type 'boolean' is not assignable to type 'string'"));
        assert!(result.contains("Type 'null' is not assignable to type 'object'"));
        assert!(result.contains("L10:"));
        assert!(result.contains("L20:"));
        assert!(result.contains("L30:"));
    }

    #[test]
    fn test_no_errors() {
        let output = "Found 0 errors. Watching for file changes.";
        let result = filter_tsc_output(output);
        assert!(result.contains("No errors found"));
    }

    #[test]
    fn test_parse_tsc_error() {
        let line = "src/file.ts(12,5): error TS2322: Type 'string' is not assignable.";
        let err = parse_tsc_error(line).unwrap();
        assert_eq!(err.file, "src/file.ts");
        assert_eq!(err.line, 12);
        assert_eq!(err.code, "TS2322");
        assert!(err.message.contains("Type 'string'"));
    }

    #[test]
    fn test_parse_tsc_error_not_tsc() {
        assert!(parse_tsc_error("normal log output").is_none());
        assert!(parse_tsc_error("src/file.ts: some other message").is_none());
    }

    #[test]
    fn test_continuation_lines() {
        let output = "src/app.tsx(10,3): error TS2322: Type '{ children: Element; }' is not assignable to type 'Props'.\n  Property 'children' does not exist on type 'Props'.\nsrc/app.tsx(20,5): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.\n";
        let result = filter_tsc_output(output);
        assert!(result.contains("Property 'children' does not exist on type 'Props'"));
        assert!(result.contains("L10:"));
        assert!(result.contains("L20:"));
    }
}
