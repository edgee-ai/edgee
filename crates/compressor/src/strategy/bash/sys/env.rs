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

//! Compressor for `env` / `printenv` command output.
//!
//! Categorizes environment variables, masks sensitive values,
//! and truncates long values to reduce token usage.

use super::BashCompressor;

pub struct EnvCompressor;

impl BashCompressor for EnvCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return Some("0 vars\n".to_string());
        }

        Some(compact_env(output))
    }
}

const SENSITIVE_PATTERNS: &[&str] = &[
    "key",
    "secret",
    "password",
    "token",
    "credential",
    "auth",
    "private",
    "api_key",
    "apikey",
    "access_key",
    "jwt",
];

fn compact_env(output: &str) -> String {
    let mut path_vars: Vec<(String, String)> = Vec::new();
    let mut lang_vars: Vec<(String, String)> = Vec::new();
    let mut tool_vars: Vec<(String, String)> = Vec::new();
    let mut other_vars: Vec<(String, String)> = Vec::new();
    let mut total = 0;

    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let (key, value) = match line.split_once('=') {
            Some((k, v)) => (k, v),
            None => continue,
        };

        total += 1;

        let is_sensitive = SENSITIVE_PATTERNS
            .iter()
            .any(|p| key.to_lowercase().contains(p));

        let display_value = if is_sensitive {
            mask_value(value)
        } else if value.len() > 100 {
            format!("{}... ({} chars)", &value[..50], value.len())
        } else {
            value.to_string()
        };

        let entry = (key.to_string(), display_value);

        if key.contains("PATH") {
            path_vars.push(entry);
        } else if is_lang_var(key) {
            lang_vars.push(entry);
        } else if is_tool_var(key) {
            tool_vars.push(entry);
        } else {
            other_vars.push(entry);
        }
    }

    let mut out = String::new();

    if !path_vars.is_empty() {
        out.push_str("PATH Variables:\n");
        for (k, v) in &path_vars {
            if k == "PATH" {
                let paths: Vec<&str> = v.split(':').collect();
                out.push_str(&format!("  PATH ({} entries):\n", paths.len()));
                for p in paths.iter().take(5) {
                    out.push_str(&format!("    {}\n", p));
                }
                if paths.len() > 5 {
                    out.push_str(&format!("    ... +{} more\n", paths.len() - 5));
                }
            } else {
                out.push_str(&format!("  {}={}\n", k, v));
            }
        }
    }

    if !lang_vars.is_empty() {
        out.push_str("\nLanguage/Runtime:\n");
        for (k, v) in &lang_vars {
            out.push_str(&format!("  {}={}\n", k, v));
        }
    }

    if !tool_vars.is_empty() {
        out.push_str("\nTools:\n");
        for (k, v) in &tool_vars {
            out.push_str(&format!("  {}={}\n", k, v));
        }
    }

    if !other_vars.is_empty() {
        out.push_str("\nOther:\n");
        for (k, v) in other_vars.iter().take(20) {
            out.push_str(&format!("  {}={}\n", k, v));
        }
        if other_vars.len() > 20 {
            out.push_str(&format!("  ... +{} more\n", other_vars.len() - 20));
        }
    }

    let shown = path_vars.len() + lang_vars.len() + tool_vars.len() + other_vars.len().min(20);
    out.push_str(&format!("\n{} vars ({} shown)\n", total, shown));

    out
}

fn mask_value(value: &str) -> String {
    if value.len() <= 4 {
        "****".to_string()
    } else {
        format!("{}****{}", &value[..2], &value[value.len() - 2..])
    }
}

fn is_lang_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "RUST", "CARGO", "PYTHON", "PIP", "NODE", "NPM", "YARN", "DENO", "BUN", "JAVA", "MAVEN",
        "GRADLE", "GO", "GOPATH", "GOROOT", "RUBY", "GEM", "PERL", "PHP", "DOTNET", "NUGET",
    ];
    let upper = key.to_uppercase();
    PATTERNS.iter().any(|p| upper.contains(p))
}

fn is_tool_var(key: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "EDITOR",
        "VISUAL",
        "SHELL",
        "TERM",
        "GIT",
        "SSH",
        "GPG",
        "BREW",
        "HOMEBREW",
        "XDG",
        "CLAUDE",
        "ANTHROPIC",
    ];
    let upper = key.to_uppercase();
    PATTERNS.iter().any(|p| upper.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_basic() {
        let input =
            "HOME=/home/user\nUSER=testuser\nSHELL=/bin/bash\nPATH=/usr/bin:/usr/local/bin\n";
        let compressor = EnvCompressor;
        let result = compressor.compress("env", input).unwrap();
        assert!(result.contains("PATH"));
        assert!(result.contains("SHELL"));
        assert!(result.contains("4 vars"));
    }

    #[test]
    fn test_compact_empty() {
        let compressor = EnvCompressor;
        let result = compressor.compress("env", "").unwrap();
        assert_eq!(result, "0 vars\n");
    }

    #[test]
    fn test_masks_sensitive() {
        let input = "API_KEY=super_secret_value\nHOME=/home/user\n";
        let compressor = EnvCompressor;
        let result = compressor.compress("env", input).unwrap();
        assert!(!result.contains("super_secret_value"));
        assert!(result.contains("****"));
    }

    #[test]
    fn test_mask_value() {
        assert_eq!(mask_value("ab"), "****");
        assert_eq!(mask_value("abcdef"), "ab****ef");
    }

    #[test]
    fn test_truncates_long_values() {
        let long_val = "x".repeat(200);
        let input = format!("SOME_VAR={}\n", long_val);
        let result = compact_env(&input);
        assert!(result.contains("200 chars"));
    }

    #[test]
    fn test_categorizes_vars() {
        let input = "RUST_LOG=debug\nEDITOR=vim\nHOME=/home/user\nPATH=/usr/bin\n";
        let result = compact_env(input);
        assert!(result.contains("Language/Runtime:"));
        assert!(result.contains("RUST_LOG"));
        assert!(result.contains("Tools:"));
        assert!(result.contains("EDITOR"));
        assert!(result.contains("PATH Variables:"));
    }

    #[test]
    fn test_path_split() {
        let input = "PATH=/usr/bin:/usr/local/bin:/home/user/bin:/opt/bin:/sbin:/usr/sbin:/extra\n";
        let result = compact_env(input);
        assert!(result.contains("7 entries"));
        assert!(result.contains("+2 more"));
    }
}
