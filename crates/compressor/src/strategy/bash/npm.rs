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

//! Compressor for `npm` / `pnpm` command output.
//!
//! Strips boilerplate lifecycle scripts, warnings, progress indicators,
//! and empty lines to produce compact output.

use super::BashCompressor;

pub struct NpmCompressor;

impl BashCompressor for NpmCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let filtered = filter_npm_output(output);
        if filtered == output {
            return None;
        }

        Some(filtered)
    }
}

/// Filter npm/pnpm output — strip boilerplate, progress bars, warnings.
fn filter_npm_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        // Skip npm lifecycle script header ("> package@version command")
        if line.starts_with('>') && line.contains('@') {
            continue;
        }
        // Skip npm warnings and notices
        if line.trim_start().starts_with("npm WARN") {
            continue;
        }
        if line.trim_start().starts_with("npm notice") {
            continue;
        }
        // Skip pnpm scope/warning lines
        if line.trim_start().starts_with("Scope:") {
            continue;
        }
        if line.trim_start().starts_with("WARN") && line.contains("deprecated") {
            continue;
        }
        // Skip progress indicators
        if line.contains('\u{2E29}') || line.contains('\u{2E28}') {
            continue;
        }
        // Skip pnpm install progress lines
        if line.contains("Progress:") || line.contains("packages in") && line.contains("reused") {
            continue;
        }
        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        result.push(line);
    }

    if result.is_empty() {
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_npm_output() {
        let output = "> project@1.0.0 build\n> next build\n\nnpm WARN deprecated inflight@1.0.6: This module is not supported\nnpm notice\n\n   Creating an optimized production build...\n   Build completed\n";
        let result = filter_npm_output(output);
        assert!(!result.contains("npm WARN"));
        assert!(!result.contains("npm notice"));
        assert!(!result.contains("> project@"));
        assert!(result.contains("Build completed"));
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_compressor_skips_clean_output() {
        let compressor = NpmCompressor;
        assert!(compressor.compress("npm run build", "").is_none());
    }

    #[test]
    fn test_filter_strips_warnings() {
        let output = "npm WARN old\nnpm WARN another\nActual output here\n";
        let result = filter_npm_output(output);
        assert!(!result.contains("npm WARN"));
        assert!(result.contains("Actual output here"));
    }

    #[test]
    fn test_pnpm_scope_lines_stripped() {
        let output = "Scope: all 6 workspace projects\n WARN  deprecated inflight@1.0.6: This module is not supported\nBuild succeeded\n";
        let result = filter_npm_output(output);
        assert!(!result.contains("Scope:"));
        assert!(!result.contains("deprecated"));
        assert!(result.contains("Build succeeded"));
    }
}
