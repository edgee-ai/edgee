//! Compressor for the Claude Code `Bash` tool output.
//!
//! Extracts the shell command from the tool call arguments JSON,
//! then delegates to the per-command compressors in `bash/`.

use super::ToolCompressor;

pub struct BashCompressor;

impl ToolCompressor for BashCompressor {
    fn compress(&self, arguments: &str, output: &str) -> Option<String> {
        let command = extract_command(arguments)?;

        // Bundled commands (&&, ||, ;) produce concatenated output from multiple
        // sub-commands with no reliable delimiters between sections. Compressing
        // would risk silently discarding output from earlier sub-commands.
        // Pipes (|) are fine — only the last command's output is captured.
        if contains_shell_operators(&command) {
            return None;
        }

        let base_command = command.split_whitespace().next().unwrap_or("");
        let compressor = crate::strategy::bash::compressor_for(base_command)?;
        compressor.compress(&command, output)
    }
}

/// Returns `true` if the command contains shell bundling operators (`&&`, `||`, `;`)
/// outside of single or double quotes. Single pipes (`|`) are not considered bundling
/// operators — piped commands produce output only from the last stage.
fn contains_shell_operators(command: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if !in_single => {
                // Skip the next character (it is escaped)
                chars.next();
            }
            ';' if !in_single && !in_double => return true,
            '&' if !in_single && !in_double => {
                if chars.peek() == Some(&'&') {
                    return true;
                }
                // Single `&` (background operator) is not a bundling operator
            }
            '|' if !in_single && !in_double => {
                if chars.peek() == Some(&'|') {
                    return true;
                }
                // Single `|` is a pipe, not a bundling operator
            }
            _ => {}
        }
    }

    false
}

/// Extract the shell command from Bash tool call arguments JSON.
/// Arguments are expected to be `{"command": "..."}`.
fn extract_command(arguments: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| match (v.get("command"), v.get("cmd")) {
            (Some(command), None) => command.as_str().map(String::from),
            (None, Some(cmd)) => cmd.as_str().map(String::from),
            (Some(command), Some(cmd)) => {
                println!("command: {:?}, cmd: {:?}", command, cmd);
                command.as_str().map(String::from)
            }
            (None, None) => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_command() {
        let args = r#"{"command": "ls -la /tmp"}"#;
        assert_eq!(extract_command(args), Some("ls -la /tmp".to_string()));
    }

    #[test]
    fn test_extract_command_missing() {
        assert_eq!(extract_command("{}"), None);
    }

    #[test]
    fn test_extract_command_invalid_json() {
        assert_eq!(extract_command("not json"), None);
    }

    #[test]
    fn test_delegates_to_bash_compressor() {
        let compressor = BashCompressor;
        let args = r#"{"command": "find . -name '*.rs'"}"#;
        let output = "src/main.rs\nsrc/lib.rs\ntests/test.rs\n";
        let result = compressor.compress(args, output);
        assert!(result.is_some());
        let compressed = result.unwrap();
        assert!(compressed.contains("3F 2D:"));
    }

    #[test]
    fn test_unknown_command_returns_none() {
        let compressor = BashCompressor;
        let args = r#"{"command": "echo hello"}"#;
        assert!(compressor.compress(args, "hello\n").is_none());
    }

    #[test]
    fn test_missing_command_returns_none() {
        let compressor = BashCompressor;
        assert!(compressor.compress("{}", "some output").is_none());
    }

    // --- contains_shell_operators tests ---

    #[test]
    fn test_no_operators() {
        assert!(!contains_shell_operators("git diff HEAD"));
    }

    #[test]
    fn test_pipe_is_not_bundling_operator() {
        assert!(!contains_shell_operators("git log | head -10"));
    }

    #[test]
    fn test_double_ampersand() {
        assert!(contains_shell_operators("git log && git diff"));
    }

    #[test]
    fn test_double_pipe() {
        assert!(contains_shell_operators("git status || echo 'failed'"));
    }

    #[test]
    fn test_semicolon() {
        assert!(contains_shell_operators("git log; git diff"));
    }

    #[test]
    fn test_operators_in_single_quotes() {
        assert!(!contains_shell_operators("echo 'a && b'"));
    }

    #[test]
    fn test_operators_in_double_quotes() {
        assert!(!contains_shell_operators(r#"echo "a && b || c ; d""#));
    }

    #[test]
    fn test_escaped_semicolon() {
        assert!(!contains_shell_operators(r"echo a\; b"));
    }

    #[test]
    fn test_mixed_quoted_and_unquoted_operators() {
        // Quoted operator should not trigger, but the unquoted one should
        assert!(contains_shell_operators(r#"echo "a && b" && git diff"#));
    }

    #[test]
    fn test_single_ampersand_is_not_bundling() {
        assert!(!contains_shell_operators("sleep 1 &"));
    }

    // --- Integration tests for bundled commands ---

    #[test]
    fn test_bundled_with_and_and_returns_none() {
        let compressor = BashCompressor;
        let args = r#"{"command": "git log --oneline -10 && git diff"}"#;
        let output = "abc1234 some commit\ndiff --git a/file b/file\n";
        assert!(compressor.compress(args, output).is_none());
    }

    #[test]
    fn test_bundled_with_semicolon_returns_none() {
        let compressor = BashCompressor;
        let args = r#"{"command": "ls -la; find . -name '*.rs'"}"#;
        let output = "total 42\ndrwxr-xr-x\n./src/main.rs\n";
        assert!(compressor.compress(args, output).is_none());
    }

    #[test]
    fn test_bundled_with_or_or_returns_none() {
        let compressor = BashCompressor;
        let args = r#"{"command": "git status || echo 'failed'"}"#;
        let output = "On branch main\n";
        assert!(compressor.compress(args, output).is_none());
    }

    #[test]
    fn test_piped_command_still_compresses() {
        let compressor = BashCompressor;
        let args = r#"{"command": "find . -name '*.rs'"}"#;
        let output = "src/main.rs\nsrc/lib.rs\ntests/test.rs\n";
        // find compressor should still work
        assert!(compressor.compress(args, output).is_some());
    }

    #[test]
    fn test_quoted_operator_in_grep_still_compresses() {
        let compressor = BashCompressor;
        // The && is inside single quotes — not a real operator
        let args = r#"{"command": "grep -rn 'a && b' src/"}"#;
        let mut output = String::new();
        for i in 1..=15 {
            output.push_str(&format!("src/file{i}.rs:10:a && b found here\n"));
        }
        assert!(compressor.compress(args, &output).is_some());
    }
}
