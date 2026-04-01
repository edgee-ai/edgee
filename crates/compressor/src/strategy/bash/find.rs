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

//! Compressor for `find` command output.
//!
//! Groups found paths by directory and adds an extension summary,
//! producing a compact listing instead of a flat file list.

use std::collections::HashMap;
use std::path::Path;

use super::BashCompressor;

pub struct FindCompressor;

impl BashCompressor for FindCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            return Some("0 results\n".to_string());
        }

        Some(compact_find(&lines))
    }
}

fn compact_find(paths: &[&str]) -> String {
    let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut by_ext: HashMap<String, usize> = HashMap::new();

    for path in paths {
        let p = Path::new(path);
        let dir = p.parent().map(|d| d.to_str().unwrap_or(".")).unwrap_or(".");
        let dir = if dir.is_empty() { "." } else { dir };
        let filename = p
            .file_name()
            .map(|f| f.to_str().unwrap_or(""))
            .unwrap_or("");

        by_dir.entry(dir).or_default().push(filename);

        let ext = p
            .extension()
            .map(|e| format!(".{}", e.to_str().unwrap_or("")))
            .unwrap_or_else(|| "no ext".to_string());
        *by_ext.entry(ext).or_default() += 1;
    }

    let mut dirs: Vec<_> = by_dir.keys().copied().collect();
    dirs.sort();

    let total = paths.len();
    let mut out = format!("{}F {}D:\n\n", total, dirs.len());

    let mut shown = 0;
    let max_results = 50;

    for dir in &dirs {
        if shown >= max_results {
            break;
        }

        let files_in_dir = &by_dir[dir];
        let dir_display = compact_path(dir);

        let remaining = max_results - shown;
        if files_in_dir.len() <= remaining {
            out.push_str(&format!("{}/ {}\n", dir_display, files_in_dir.join(" ")));
            shown += files_in_dir.len();
        } else {
            let partial: Vec<&str> = files_in_dir.iter().take(remaining).copied().collect();
            out.push_str(&format!("{}/ {}\n", dir_display, partial.join(" ")));
            shown += partial.len();
            break;
        }
    }

    if shown < total {
        out.push_str(&format!("+{} more\n", total - shown));
    }

    // Extension summary
    if by_ext.len() > 1 {
        let mut exts: Vec<_> = by_ext.iter().collect();
        exts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let ext_parts: Vec<String> = exts
            .iter()
            .take(5)
            .map(|(e, c)| format!("{}({})", e, c))
            .collect();
        out.push_str(&format!("\next: {}\n", ext_parts.join(" ")));
    }

    out
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }
    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_basic() {
        let input = "src/main.rs\nsrc/lib.rs\ntests/test.rs\n";
        let compressor = FindCompressor;
        let result = compressor.compress("find . -name '*.rs'", input).unwrap();
        assert!(result.contains("3F 2D:"));
        assert!(result.contains("src/"));
        assert!(result.contains("main.rs"));
        assert!(result.contains("lib.rs"));
        assert!(result.contains("tests/"));
        assert!(result.contains("test.rs"));
    }

    #[test]
    fn test_compact_empty() {
        let compressor = FindCompressor;
        let result = compressor.compress("find . -name '*.xyz'", "").unwrap();
        assert_eq!(result, "0 results\n");
    }

    #[test]
    fn test_compact_single_dir() {
        let result = compact_find(&["main.rs", "lib.rs", "utils.rs"]);
        assert!(result.contains("3F 1D:"));
        assert!(result.contains("./ main.rs lib.rs utils.rs"));
    }

    #[test]
    fn test_compact_extension_summary() {
        let input = vec!["src/main.rs", "src/lib.rs", "Cargo.toml", "README.md"];
        let result = compact_find(&input);
        assert!(result.contains("ext:"));
        assert!(result.contains(".rs(2)"));
    }

    #[test]
    fn test_compact_path_short() {
        assert_eq!(compact_path("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn test_compact_path_long() {
        let long = "very/long/deeply/nested/path/to/some/directory/here";
        let result = compact_path(long);
        assert!(result.contains("..."));
        assert!(result.len() <= long.len());
    }

    #[test]
    fn test_compact_many_results() {
        let paths: Vec<String> = (0..100).map(|i| format!("src/file{}.rs", i)).collect();
        let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        let result = compact_find(&path_refs);
        assert!(result.contains("100F"));
        assert!(result.contains("+50 more"));
    }
}
