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

//! Compressor for `tree` command output.
//!
//! Removes summary lines and trailing empty lines to reduce token usage
//! while preserving the directory structure visualization.

use super::BashCompressor;

/// Directories that are noise for LLM context.
const NOISE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    "env",
    ".env",
    "coverage",
    ".nyc_output",
    ".DS_Store",
    "Thumbs.db",
    ".idea",
    ".vscode",
    ".vs",
    "*.egg-info",
    ".eggs",
];

pub struct TreeCompressor;

impl BashCompressor for TreeCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        // Always compress tree output (no format detection needed like ls -l)
        Some(filter_tree_output(output))
    }
}

fn filter_tree_output(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    if lines.is_empty() {
        return "\n".to_string();
    }

    let mut filtered_lines = Vec::new();
    let mut skip_depth: Option<usize> = None;

    for line in lines {
        // Skip the final summary line (e.g., "5 directories, 23 files")
        if line.contains("director") && line.contains("file") {
            continue;
        }

        // Skip empty lines at the end
        if line.trim().is_empty() && filtered_lines.is_empty() {
            continue;
        }

        // Calculate indentation depth (number of tree characters before content)
        let depth = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '│' || *c == '├' || *c == '└' || *c == '─')
            .count();

        // If we're skipping a noise directory, skip all nested content
        if let Some(skip_d) = skip_depth {
            if depth > skip_d {
                continue; // Still inside the noise directory
            } else {
                skip_depth = None; // Exited the noise directory
            }
        }

        // Extract the actual filename/dirname from tree's formatted output
        let trimmed = line
            .trim_start_matches(|c: char| {
                c.is_whitespace() || c == '│' || c == '├' || c == '└' || c == '─'
            })
            .trim();

        // Check if this line is a noise directory
        let is_noise = NOISE_DIRS.iter().any(|noise| {
            // Check exact match or wildcard pattern match
            if noise.starts_with('*') {
                let suffix = noise.trim_start_matches('*');
                trimmed.ends_with(suffix)
            } else {
                trimmed == *noise || trimmed.starts_with(&format!("{}/", noise))
            }
        });

        if is_noise {
            skip_depth = Some(depth); // Start skipping this directory and its children
            continue;
        }

        filtered_lines.push(line);
    }

    // Remove trailing empty lines
    while filtered_lines.last().is_some_and(|l| l.trim().is_empty()) {
        filtered_lines.pop();
    }

    filtered_lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_removes_summary() {
        let input = ".\n├── src\n│   └── main.rs\n└── Cargo.toml\n\n2 directories, 3 files\n";
        let output = filter_tree_output(input);
        assert!(!output.contains("directories"));
        assert!(!output.contains("files"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("Cargo.toml"));
    }

    #[test]
    fn test_filter_preserves_structure() {
        let input = ".\n├── src\n│   ├── main.rs\n│   └── lib.rs\n└── tests\n    └── test.rs\n";
        let output = filter_tree_output(input);
        assert!(output.contains("├──"));
        assert!(output.contains("│"));
        assert!(output.contains("└──"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("test.rs"));
    }

    #[test]
    fn test_filter_handles_empty() {
        let input = "";
        let output = filter_tree_output(input);
        assert_eq!(output, "\n");
    }

    #[test]
    fn test_filter_removes_trailing_empty_lines() {
        let input = ".\n├── file.txt\n\n\n";
        let output = filter_tree_output(input);
        assert_eq!(output.matches('\n').count(), 2); // Root + file.txt + final newline
    }

    #[test]
    fn test_filter_summary_variations() {
        // Test different summary formats
        let inputs = vec![
            (".\n└── file.txt\n\n0 directories, 1 file\n", "1 file"),
            (".\n└── file.txt\n\n1 directory, 0 files\n", "1 directory"),
            (".\n└── file.txt\n\n10 directories, 25 files\n", "25 files"),
        ];

        for (input, summary_fragment) in inputs {
            let output = filter_tree_output(input);
            assert!(
                !output.contains(summary_fragment),
                "Should remove summary '{}' from output",
                summary_fragment
            );
            assert!(
                output.contains("file.txt"),
                "Should preserve file.txt in output"
            );
        }
    }

    #[test]
    fn test_noise_dirs_constant() {
        // Verify NOISE_DIRS contains expected patterns
        assert!(NOISE_DIRS.contains(&"node_modules"));
        assert!(NOISE_DIRS.contains(&".git"));
        assert!(NOISE_DIRS.contains(&"target"));
        assert!(NOISE_DIRS.contains(&"__pycache__"));
        assert!(NOISE_DIRS.contains(&".next"));
        assert!(NOISE_DIRS.contains(&"dist"));
        assert!(NOISE_DIRS.contains(&"build"));
    }

    #[test]
    fn test_compressor_compresses_tree_output() {
        let compressor = TreeCompressor;
        let input = ".\n├── src\n│   └── main.rs\n└── Cargo.toml\n\n2 directories, 3 files\n";
        let result = compressor.compress("tree", input);
        assert!(result.is_some());
        let output = result.unwrap();
        assert!(output.contains("src"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("Cargo.toml"));
        assert!(!output.contains("directories"));
        assert!(!output.contains("files"));
    }

    #[test]
    fn test_compressor_handles_empty_output() {
        let compressor = TreeCompressor;
        let result = compressor.compress("tree", "");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "\n");
    }

    #[test]
    fn test_compressor_preserves_structure_chars() {
        let compressor = TreeCompressor;
        let input = ".\n├── dir1\n│   ├── file1.txt\n│   └── file2.txt\n└── dir2\n    └── file3.txt\n\n2 directories, 3 files\n";
        let result = compressor.compress("tree -L 2", input);
        assert!(result.is_some());
        let output = result.unwrap();
        assert!(output.contains("├──"));
        assert!(output.contains("│"));
        assert!(output.contains("└──"));
    }

    #[test]
    fn test_filter_removes_noise_dirs() {
        let input = ".\n├── src\n│   └── main.rs\n├── node_modules\n│   └── package\n├── target\n│   └── debug\n└── .git\n    └── config\n\n5 directories, 3 files\n";
        let output = filter_tree_output(input);
        assert!(output.contains("src"));
        assert!(output.contains("main.rs"));
        assert!(!output.contains("node_modules"));
        assert!(!output.contains("target"));
        assert!(!output.contains(".git"));
    }

    #[test]
    fn test_filter_removes_noise_dirs_nested() {
        let input = ".\n├── src\n│   ├── main.rs\n│   └── lib.rs\n├── dist\n│   ├── bundle.js\n│   └── bundle.css\n├── .next\n│   └── cache\n└── build\n    └── output\n";
        let output = filter_tree_output(input);
        assert!(output.contains("src"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("lib.rs"));
        assert!(!output.contains("dist"));
        assert!(!output.contains("bundle.js"));
        assert!(!output.contains(".next"));
        assert!(!output.contains("build"));
    }

    #[test]
    fn test_filter_removes_wildcard_patterns() {
        let input = ".\n├── src\n│   └── main.rs\n├── mypackage.egg-info\n│   └── PKG-INFO\n└── .eggs\n    └── package\n";
        let output = filter_tree_output(input);
        assert!(output.contains("src"));
        assert!(output.contains("main.rs"));
        assert!(!output.contains(".egg-info"));
        assert!(!output.contains(".eggs"));
    }

    #[test]
    fn test_filter_preserves_similar_names() {
        // Ensure we don't accidentally filter legitimate directories
        let input = ".\n├── src\n│   └── main.rs\n├── targets\n│   └── file.txt\n└── node_modules_backup\n    └── package.json\n";
        let output = filter_tree_output(input);
        // These should be preserved because they're not exact matches
        assert!(output.contains("targets"));
        assert!(output.contains("node_modules_backup"));
    }
}
