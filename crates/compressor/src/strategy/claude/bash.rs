//! Compressor for the Claude Code `Bash` tool output.
//!
//! Extracts the shell command from the tool call arguments JSON,
//! then delegates to the per-command compressors in `bash/`.

use super::ToolCompressor;

pub struct BashCompressor;

impl ToolCompressor for BashCompressor {
    fn compress(&self, arguments: &str, output: &str) -> Option<String> {
        let command = extract_command(arguments)?;

        // Strip prefixes that don't change the program being run:
        //   - leading env-var assignments (`FOO=bar cargo build`)
        //   - keyword wrappers (`sudo`, `time`, `nohup`, `exec`, `env`)
        //   - silent leading sub-commands (`cd path && cargo build`,
        //     `export X=1 && cargo build`)
        // After unwrapping, the dispatch target is the inner command.
        let dispatch_target = extract_dispatch_target(&command)?;

        // If shell bundling operators remain after unwrapping, give up:
        // outputs from multiple commands are concatenated with no reliable
        // separator, so compressing would risk silently dropping output from
        // earlier sub-commands.
        if contains_shell_operators(dispatch_target) {
            return None;
        }

        // Dispatch on the *basename* of the first token so absolute paths
        // (`/usr/local/bin/cargo`, `~/bin/cargo`) work the same as bare
        // command names.
        let first_token = dispatch_target.split_whitespace().next().unwrap_or("");
        let basename = first_token.rsplit('/').next().unwrap_or(first_token);
        let compressor = crate::strategy::bash::compressor_for(basename)?;
        compressor.compress(dispatch_target, output)
    }
}

/// Iteratively strip recognized command-line prefixes (env assignments,
/// keyword wrappers, silent leading sub-commands) and return the first
/// non-prefix sub-command. Returns `None` for an empty result.
///
/// "Silent leading sub-commands" cover the common `cd path && real_cmd`
/// pattern: a known no-output command followed by `&&` / `;` / `||`.
/// Stripping those means a tool call like `cd src && cargo build` still
/// gets routed to the cargo compressor instead of falling through to "no
/// compression" because of the `&&`.
fn extract_dispatch_target(command: &str) -> Option<&str> {
    let mut rest = command.trim();

    loop {
        let after_assigns = strip_leading_assignments(rest);
        if after_assigns != rest {
            rest = after_assigns;
            continue;
        }

        let after_keyword = strip_keyword_prefix(rest);
        if after_keyword != rest {
            rest = after_keyword;
            continue;
        }

        let after_silent = strip_silent_chain_prefix(rest);
        if after_silent != rest {
            rest = after_silent;
            continue;
        }

        break;
    }

    if rest.is_empty() { None } else { Some(rest) }
}

/// Strip leading `VAR=value` shell assignments. The match is intentionally
/// conservative: the variable name must be a valid identifier and the token
/// must not start with `-` (so `--flag=value` is left alone).
fn strip_leading_assignments(input: &str) -> &str {
    let mut rest = input.trim_start();
    loop {
        let token_end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        if token_end == 0 {
            return rest;
        }
        let token = &rest[..token_end];
        if let Some(eq) = token.find('=')
            && eq > 0
            && !token.starts_with('-')
            && token[..eq]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && token[..eq]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            rest = rest[token_end..].trim_start();
            continue;
        }
        return rest;
    }
}

/// Strip a leading wrapper keyword such as `sudo`, `time`, `env`, `nohup`,
/// `exec`. Returns the input unchanged if no recognized keyword is present.
fn strip_keyword_prefix(input: &str) -> &str {
    let trimmed = input.trim_start();
    let token_end = trimmed
        .find(|c: char| c.is_whitespace())
        .unwrap_or(trimmed.len());
    let token = &trimmed[..token_end];
    let basename = token.rsplit('/').next().unwrap_or(token);
    if matches!(basename, "sudo" | "time" | "nohup" | "exec" | "env") {
        trimmed[token_end..].trim_start()
    } else {
        input
    }
}

/// If the command starts with a known no-output sub-command followed by a
/// top-level `&&` / `;` / `||`, return the part *after* the operator. Used
/// to peel off `cd path &&`, `export X=1 &&`, etc.
fn strip_silent_chain_prefix(input: &str) -> &str {
    let trimmed = input.trim_start();
    let Some(op_start) = find_top_level_operator(trimmed) else {
        return input;
    };
    let head = trimmed[..op_start].trim();
    if !is_silent_command(head) {
        return input;
    }
    let after_op = match trimmed[op_start..].chars().next() {
        Some('&') | Some('|') => &trimmed[op_start + 2..], // && or ||
        Some(';') => &trimmed[op_start + 1..],
        _ => return input,
    };
    after_op.trim_start()
}

/// Returns `true` for sub-commands that don't produce output we'd want to
/// compress (so they're safe to peel off the front of a `&&`-chain).
fn is_silent_command(segment: &str) -> bool {
    let first = segment.split_whitespace().next().unwrap_or("");
    let basename = first.rsplit('/').next().unwrap_or(first);
    matches!(
        basename,
        "cd" | "export"
            | "set"
            | "unset"
            | "alias"
            | "unalias"
            | "true"
            | "false"
            | ":"
            | "shopt"
            | "umask"
            | "pushd"
            | "popd"
    )
}

/// Return the byte position of the first top-level `&&`, `||`, or `;`
/// operator in `input`. Quoted/escaped operators are ignored, mirroring
/// the rules in [`contains_shell_operators`].
fn find_top_level_operator(input: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'\\' if !in_single && i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b';' if !in_single && !in_double => return Some(i),
            b'&' if !in_single && !in_double && bytes.get(i + 1) == Some(&b'&') => {
                return Some(i);
            }
            b'|' if !in_single && !in_double && bytes.get(i + 1) == Some(&b'|') => {
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Returns `true` if the command contains shell bundling operators (`&&`, `||`, `;`)
/// outside of single or double quotes. Single pipes (`|`) are not considered bundling
/// operators — piped commands produce output only from the last stage.
fn contains_shell_operators(command: &str) -> bool {
    find_top_level_operator(command).is_some()
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
                tracing::debug!(
                    ?command,
                    ?cmd,
                    "bash: both 'command' and 'cmd' keys present, using 'command'"
                );
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

    // --- Dispatch-target extraction tests ---

    #[test]
    fn dispatch_strips_var_assignments() {
        assert_eq!(
            extract_dispatch_target("FOO=bar BAZ=qux cargo build"),
            Some("cargo build")
        );
    }

    #[test]
    fn dispatch_strips_sudo_prefix() {
        assert_eq!(
            extract_dispatch_target("sudo cargo build"),
            Some("cargo build")
        );
    }

    #[test]
    fn dispatch_strips_time_prefix() {
        assert_eq!(extract_dispatch_target("time ls -la"), Some("ls -la"));
    }

    #[test]
    fn dispatch_strips_env_prefix_and_assignments() {
        assert_eq!(
            extract_dispatch_target("env FOO=1 BAR=2 cargo test"),
            Some("cargo test")
        );
    }

    #[test]
    fn dispatch_strips_silent_cd_chain() {
        assert_eq!(
            extract_dispatch_target("cd src && cargo build"),
            Some("cargo build")
        );
    }

    #[test]
    fn dispatch_strips_chained_silent_prefixes() {
        // cd ... && export X=1 && cargo test
        assert_eq!(
            extract_dispatch_target("cd src && export X=1 && cargo test"),
            Some("cargo test")
        );
    }

    #[test]
    fn dispatch_does_not_strip_when_first_segment_is_loud() {
        // ls produces output we'd want to compress; do NOT strip the first segment.
        assert_eq!(
            extract_dispatch_target("ls -la && cargo build"),
            Some("ls -la && cargo build")
        );
    }

    #[test]
    fn dispatch_basename_routes_absolute_path_command() {
        let compressor = BashCompressor;
        let args = r#"{"command": "/usr/local/bin/find . -name '*.rs'"}"#;
        let output = "src/main.rs\nsrc/lib.rs\ntests/test.rs\n";
        let result = compressor.compress(args, output);
        assert!(
            result.is_some(),
            "absolute-path commands should dispatch by basename"
        );
    }

    #[test]
    fn dispatch_combo_sudo_path_cd_chain() {
        // sudo /usr/bin/env CARGO_TERM_COLOR=never cd src && cargo build
        // After all stripping the dispatch target is `cargo build`.
        let target = extract_dispatch_target(
            "sudo /usr/bin/env CARGO_TERM_COLOR=never cd src && cargo build",
        )
        .unwrap();
        // We don't require an exact string here — just that it routes to cargo.
        let basename = target
            .split_whitespace()
            .next()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap();
        assert_eq!(basename, "cargo");
    }

    #[test]
    fn is_silent_recognizes_known_no_output_commands() {
        assert!(is_silent_command("cd src"));
        assert!(is_silent_command("export FOO=bar"));
        assert!(is_silent_command(":"));
        assert!(is_silent_command("/usr/bin/cd somewhere"));
        assert!(!is_silent_command("ls -la"));
        assert!(!is_silent_command("cargo build"));
    }
}
