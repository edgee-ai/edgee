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

//! Compressor for `diff` / `git diff` unified diff output.
//!
//! Condenses unified diff format into per-file summaries with only
//! the changed lines, stripping context lines and headers.

use super::BashCompressor;

const MAX_CHANGES_PER_FILE: usize = 15;

pub struct DiffCompressor;

impl BashCompressor for DiffCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let result = condense_unified_diff(output);
        if result.is_empty() {
            return None;
        }

        Some(result)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..s.floor_char_boundary(max.saturating_sub(3))])
    }
}

fn condense_unified_diff(diff: &str) -> String {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current_file = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut changes: Vec<String> = Vec::new();

    for line in diff.lines() {
        if line.starts_with("diff --git") || line.starts_with("--- ") || line.starts_with("+++ ") {
            if line.starts_with("+++ ") {
                // Flush previous file
                if !current_file.is_empty() && (added > 0 || removed > 0) {
                    files.push(FileDiff {
                        path: current_file.clone(),
                        added,
                        removed,
                        changes: std::mem::take(&mut changes),
                    });
                }
                current_file = line
                    .trim_start_matches("+++ ")
                    .trim_start_matches("b/")
                    .to_string();
                added = 0;
                removed = 0;
                changes.clear();
            }
        } else if line.starts_with("index ") || line.starts_with("@@") {
            continue;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
            if changes.len() < MAX_CHANGES_PER_FILE {
                changes.push(truncate(line, 80));
            }
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
            if changes.len() < MAX_CHANGES_PER_FILE {
                changes.push(truncate(line, 80));
            }
        }
    }

    // Flush last file
    if !current_file.is_empty() && (added > 0 || removed > 0) {
        files.push(FileDiff {
            path: current_file,
            added,
            removed,
            changes,
        });
    }

    if files.is_empty() {
        return String::new();
    }

    let total_added: usize = files.iter().map(|f| f.added).sum();
    let total_removed: usize = files.iter().map(|f| f.removed).sum();

    let mut out = format!("{}F +{} -{}\n\n", files.len(), total_added, total_removed);

    for f in &files {
        out.push_str(&format!("{} (+{} -{})\n", f.path, f.added, f.removed));
        for c in f.changes.iter().take(10) {
            out.push_str(&format!("  {}\n", c));
        }
        if f.changes.len() > 10 {
            out.push_str(&format!("  ... +{} more\n", f.changes.len() - 10));
        }
        out.push('\n');
    }

    out
}

struct FileDiff {
    path: String,
    added: usize,
    removed: usize,
    changes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_condense_single_file() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("hello");
     println!("world");
 }
"#;
        let compressor = DiffCompressor;
        let result = compressor.compress("git diff", diff).unwrap();
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("+1 -0"));
        assert!(result.contains("println"));
    }

    #[test]
    fn test_condense_multiple_files() {
        let diff = r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1,2 @@
 existing
+added line
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,2 +1 @@
-removed line
 kept
"#;
        let compressor = DiffCompressor;
        let result = compressor.compress("git diff", diff).unwrap();
        assert!(result.contains("2F"));
        assert!(result.contains("a.rs"));
        assert!(result.contains("b.rs"));
    }

    #[test]
    fn test_condense_empty() {
        let compressor = DiffCompressor;
        assert!(compressor.compress("git diff", "").is_none());
    }

    #[test]
    fn test_condense_no_changes() {
        let diff = "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n";
        let compressor = DiffCompressor;
        assert!(compressor.compress("diff", diff).is_none());
    }

    #[test]
    fn test_condense_summary_counts() {
        let diff = r#"diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,5 @@
+line1
+line2
-old1
 unchanged
"#;
        let result = condense_unified_diff(diff);
        assert!(result.contains("+2 -1"));
        assert!(result.contains("1F +2 -1"));
    }

    #[test]
    fn test_truncate_long_lines() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world!", 8), "hello...");
    }

    #[test]
    fn test_context_lines_stripped() {
        let diff = r#"diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -1,5 +1,5 @@
 context1
 context2
-old
+new
 context3
 context4
"#;
        let result = condense_unified_diff(diff);
        // Should only contain changed lines, not context
        assert!(!result.contains("context1"));
        assert!(result.contains("-old"));
        assert!(result.contains("+new"));
    }
}
