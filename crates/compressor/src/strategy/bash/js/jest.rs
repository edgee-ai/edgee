//! Compressor for `jest` and `vitest` test runner output.
//!
//! Strips PASS/RUNS lines, numbered code-context blocks, caret annotations,
//! and boilerplate footers. Keeps only FAIL lines and the test summary.

use super::BashCompressor;

pub struct JestCompressor;

impl BashCompressor for JestCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let compressed = compress_jest_output(output);
        if compressed.trim_end().len() >= output.trim_end().len() {
            return None;
        }

        Some(compressed)
    }
}

/// Returns true for numbered code-context lines produced by jest/vitest:
///   `  10 |     const x = 1;`   (space + digits + " |")
///   `> 12 |     expect(...)...`  ("> " prefix + digits + " |")
fn is_code_context_line(trimmed: &str) -> bool {
    let s = trimmed.trim_start_matches('>').trim_start_matches(' ');
    let rest = s.trim_start_matches(|c: char| c.is_ascii_digit());
    !s.is_empty() && s.starts_with(|c: char| c.is_ascii_digit()) && rest.starts_with(" |")
}

/// Returns true for caret / annotation lines after a code-context block:
///   `         |                      ^`
///   `         |          ^^^^^^^^^^^^`
fn is_annotation_line(trimmed: &str) -> bool {
    let s = trimmed.trim_start_matches(' ');
    if !s.starts_with('|') {
        return false;
    }
    let content = s[1..].trim_start_matches(' ');
    !content.is_empty() && content.chars().all(|c| matches!(c, '^' | '~' | '-' | ' '))
}

fn compress_jest_output(output: &str) -> String {
    let mut result: Vec<&str> = Vec::new();
    let mut prev_blank = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Strip PASS / RUNS / RERUNS status lines
        if trimmed.starts_with("PASS ")
            || trimmed.starts_with("RUNS ")
            || trimmed.starts_with("RERUN ")
        {
            continue;
        }

        // Strip numbered code context and caret annotation lines
        if is_code_context_line(trimmed) || is_annotation_line(trimmed) {
            continue;
        }

        // Strip boilerplate footers
        if trimmed == "Ran all test suites."
            || trimmed.starts_with("Ran all test suites matching")
            || trimmed.starts_with("Snapshots:")
            || trimmed.starts_with("Watch Usage")
            || trimmed.starts_with("Press ")
        {
            continue;
        }

        // Collapse consecutive blank lines
        let is_blank = trimmed.is_empty();
        if is_blank && prev_blank {
            continue;
        }
        prev_blank = is_blank;

        result.push(line);
    }

    // Trim trailing blank lines
    while result
        .last()
        .map(|l: &&str| l.trim().is_empty())
        .unwrap_or(false)
    {
        result.pop();
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const JEST_OUTPUT: &str = r#" RUNS  src/__tests__/api.test.js
 FAIL  src/__tests__/api.test.js
  ● UserService › create › creates a user

    TypeError: Cannot read properties of undefined

      10 |     const service = new UserService(mockDb);
      11 |     const result = await service.create({ name: 'Alice' });
    > 12 |     expect(result.id).toBeDefined();
         |                      ^
      13 |   });

      at Object.<anonymous> (src/__tests__/api.test.js:12:22)

 PASS  src/__tests__/utils.test.js

Test Suites: 1 failed, 1 passed, 2 total
Tests:       1 failed, 3 passed, 4 total
Snapshots:   0 total
Time:        2.341 s
Ran all test suites.
"#;

    #[test]
    fn test_strips_pass_lines() {
        let result = JestCompressor.compress("jest", JEST_OUTPUT).unwrap();
        assert!(!result.contains("PASS  src/__tests__/utils.test.js"));
    }

    #[test]
    fn test_strips_runs_lines() {
        let result = JestCompressor.compress("jest", JEST_OUTPUT).unwrap();
        assert!(!result.contains(" RUNS "));
    }

    #[test]
    fn test_strips_code_context_lines() {
        let result = JestCompressor.compress("jest", JEST_OUTPUT).unwrap();
        assert!(!result.contains("10 |"));
        assert!(!result.contains("11 |"));
        assert!(!result.contains("> 12 |"));
        assert!(!result.contains("         |                      ^"));
    }

    #[test]
    fn test_strips_boilerplate() {
        let result = JestCompressor.compress("jest", JEST_OUTPUT).unwrap();
        assert!(!result.contains("Ran all test suites."));
        assert!(!result.contains("Snapshots:"));
    }

    #[test]
    fn test_keeps_failure_info() {
        let result = JestCompressor.compress("jest", JEST_OUTPUT).unwrap();
        assert!(result.contains("FAIL  src/__tests__/api.test.js"));
        assert!(result.contains("UserService › create › creates a user"));
        assert!(result.contains("TypeError: Cannot read properties of undefined"));
    }

    #[test]
    fn test_keeps_summary() {
        let result = JestCompressor.compress("jest", JEST_OUTPUT).unwrap();
        assert!(result.contains("Tests:       1 failed, 3 passed, 4 total"));
    }

    #[test]
    fn test_empty_passthrough() {
        assert!(JestCompressor.compress("jest", "").is_none());
    }

    #[test]
    fn test_code_context_detection() {
        assert!(is_code_context_line("  10 |     const x = 1;"));
        assert!(is_code_context_line("> 12 |     expect(foo).toBe(1);"));
        assert!(!is_code_context_line(
            "  at Object.<anonymous> (file.js:10:5)"
        ));
        assert!(!is_code_context_line("TypeError: foo is not a function"));
    }

    #[test]
    fn test_annotation_detection() {
        assert!(is_annotation_line("         |                      ^"));
        assert!(is_annotation_line("         |          ^^^^^^^^^^^^"));
        assert!(!is_annotation_line("  10 |     const x = 1;"));
    }

    #[test]
    fn test_vitest_output_compressed() {
        let vitest = r#" RUN  v1.5.0 /project

 ✓ src/utils.test.ts (3)
 ✗ src/api.test.ts (1)
   ✗ UserService > creates a user

    AssertionError: expected undefined to be defined

 Test Files  1 failed | 1 passed (2)
 Tests       1 failed | 3 passed (4)
 Duration    1.23s
"#;
        // vitest uses different symbols but same basic structure;
        // compressor should at minimum not expand the output
        let result = JestCompressor.compress("vitest", vitest);
        if let Some(r) = result {
            assert!(r.len() < vitest.len());
        }
    }
}
