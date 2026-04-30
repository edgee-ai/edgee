//! `edgee statusline --wrap <command>` — generic merge wrapper.
//!
//! Reads stdin once, runs Edgee's renderer in-process and the wrapped command
//! through the platform shell, waits for both with a timeout, and merges the
//! outputs into a single line. Edgee's segment is always emitted first (or
//! last, when `EDGEE_STATUSLINE_POSITION=right`) and is never truncated; the
//! wrapped command's output gets the remaining width budget.
//!
//! The wrapper must never crash the statusline: any unexpected error falls
//! back to printing Edgee's segment alone.

use std::io::Read;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::render;
use super::width::{display_width, truncate_display};

const DEFAULT_TIMEOUT_MS: u64 = 2000;
const DEFAULT_SEPARATOR: &str = " │ ";
const DEFAULT_FALLBACK_COLUMNS: usize = 200;
const DEFAULT_MIN_WRAPPED_WIDTH: usize = 10;
const ELLIPSIS: &str = "…";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Left,
    Right,
}

impl Position {
    fn from_env() -> Self {
        match std::env::var("EDGEE_STATUSLINE_POSITION").ok().as_deref() {
            Some("right") => Self::Right,
            _ => Self::Left,
        }
    }
}

/// Public entrypoint for the `--wrap` flag.
pub async fn run(command: String) -> Result<()> {
    let stdin = read_stdin();
    let line = run_merge(command, stdin).await;
    println!("{line}");
    Ok(())
}

fn read_stdin() -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = std::io::stdin().lock().read_to_end(&mut buf);
    buf
}

async fn run_merge(command: String, stdin: Vec<u8>) -> String {
    let timeout = Duration::from_millis(parse_env_u64(
        "EDGEE_STATUSLINE_TIMEOUT_MS",
        DEFAULT_TIMEOUT_MS,
    ));
    let separator =
        std::env::var("EDGEE_STATUSLINE_SEPARATOR").unwrap_or_else(|_| DEFAULT_SEPARATOR.to_string());
    let position = Position::from_env();
    let columns = detect_columns();
    let min_wrapped = parse_env_usize(
        "EDGEE_STATUSLINE_MIN_WRAPPED_WIDTH",
        DEFAULT_MIN_WRAPPED_WIDTH,
    );
    let pass_stderr = matches!(
        std::env::var("EDGEE_STATUSLINE_PASS_STDERR").as_deref(),
        Ok("1") | Ok("true")
    );

    // Run both producers in parallel, gated by a single shared timeout.
    let edgee_fut = render::render_line();
    let wrapped_fut = run_wrapped(command.clone(), stdin.clone(), pass_stderr);

    let race = tokio::time::timeout(timeout, async {
        tokio::join!(edgee_fut, wrapped_fut)
    })
    .await;

    let (edgee_out, wrapped_out) = match race {
        Ok((edgee, wrapped)) => (edgee, wrapped.ok()),
        Err(_) => {
            // Total timeout: try to recover Edgee at least.
            let edgee = tokio::time::timeout(Duration::from_millis(50), render::render_line())
                .await
                .unwrap_or_else(|_| String::new());
            (edgee, None)
        }
    };

    merge_outputs(MergeInputs {
        edgee: trim_to_one_line(&edgee_out),
        wrapped: wrapped_out.as_deref().map(trim_to_one_line),
        separator: &separator,
        position,
        columns,
        min_wrapped_width: min_wrapped,
    })
}

async fn run_wrapped(command: String, stdin: Vec<u8>, pass_stderr: bool) -> Result<String> {
    if command.trim().is_empty() {
        anyhow::bail!("wrapped command is empty");
    }

    let mut cmd = shell_command(&command);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    cmd.stderr(if pass_stderr {
        Stdio::inherit()
    } else {
        Stdio::null()
    });

    let mut child = cmd.spawn()?;

    if let Some(mut child_stdin) = child.stdin.take() {
        let _ = child_stdin.write_all(&stdin).await;
        // Drop closes the pipe, signaling EOF to the child.
        drop(child_stdin);
    }

    let output = child.wait_with_output().await?;
    if !output.status.success() {
        anyhow::bail!("wrapped command exited non-zero: {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let mut c = Command::new("/bin/sh");
    c.arg("-c").arg(command);
    c
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut c = Command::new("cmd.exe");
    c.arg("/C").arg(command);
    c
}

fn trim_to_one_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim_end().to_string()
}

fn detect_columns() -> usize {
    if let Ok(s) = std::env::var("COLUMNS") {
        if let Ok(n) = s.trim().parse::<usize>() {
            if n > 0 {
                return n;
            }
        }
    }
    #[cfg(unix)]
    {
        if let Ok(out) = std::process::Command::new("tput").arg("cols").output() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                if let Ok(n) = s.trim().parse::<usize>() {
                    if n > 0 {
                        return n;
                    }
                }
            }
        }
    }
    DEFAULT_FALLBACK_COLUMNS
}

fn parse_env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn parse_env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

pub(crate) struct MergeInputs<'a> {
    pub edgee: String,
    pub wrapped: Option<String>,
    pub separator: &'a str,
    pub position: Position,
    pub columns: usize,
    pub min_wrapped_width: usize,
}

pub(crate) fn merge_outputs(input: MergeInputs<'_>) -> String {
    let edgee = input.edgee;
    let wrapped = input
        .wrapped
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    // Edgee precedence guarantee: when wrapped is missing or empty, render
    // Edgee alone with no orphan separator.
    let Some(wrapped) = wrapped else {
        return edgee;
    };

    let edgee_width = display_width(&edgee);
    let separator_width = display_width(input.separator);
    let total_required = edgee_width.saturating_add(separator_width);

    // Reserve a 1-cell margin so the terminal can place a cursor after the
    // statusline without wrapping. Matches Claude Code's typical rendering.
    let margin = 1usize;
    let budget = input
        .columns
        .saturating_sub(total_required)
        .saturating_sub(margin);

    if budget < input.min_wrapped_width {
        return edgee;
    }

    let truncated = truncate_display(&wrapped, budget, ELLIPSIS);
    if truncated.is_empty() {
        return edgee;
    }

    match input.position {
        Position::Left => format!("{edgee}{}{truncated}", input.separator),
        Position::Right => format!("{truncated}{}{edgee}", input.separator),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(
        edgee: &str,
        wrapped: Option<&str>,
        separator: &'a str,
        position: Position,
        columns: usize,
    ) -> MergeInputs<'a> {
        MergeInputs {
            edgee: edgee.to_string(),
            wrapped: wrapped.map(str::to_string),
            separator,
            position,
            columns,
            min_wrapped_width: DEFAULT_MIN_WRAPPED_WIDTH,
        }
    }

    #[test]
    fn merge_no_wrapped_emits_edgee_alone() {
        let s = merge_outputs(inputs("EDGEE", None, " | ", Position::Left, 80));
        assert_eq!(s, "EDGEE");
    }

    #[test]
    fn merge_empty_wrapped_emits_edgee_alone() {
        let s = merge_outputs(inputs("EDGEE", Some(""), " | ", Position::Left, 80));
        assert_eq!(s, "EDGEE");
    }

    #[test]
    fn merge_both_present_left_position() {
        let s = merge_outputs(inputs("EDGEE", Some("OTHER"), " | ", Position::Left, 80));
        assert_eq!(s, "EDGEE | OTHER");
    }

    #[test]
    fn merge_both_present_right_position() {
        let s = merge_outputs(inputs("EDGEE", Some("OTHER"), " | ", Position::Right, 80));
        assert_eq!(s, "OTHER | EDGEE");
    }

    #[test]
    fn merge_truncates_wrapped_to_fit_columns() {
        // columns=20, edgee=5 ("EDGEE"), sep=" | "(3), margin=1 → budget=11
        // Wrapped "0123456789ABCDEF" (16) → truncated to 11-1=10 chars + "…"
        let s = merge_outputs(inputs(
            "EDGEE",
            Some("0123456789ABCDEF"),
            " | ",
            Position::Left,
            20,
        ));
        assert!(s.starts_with("EDGEE | "));
        assert!(s.ends_with(ELLIPSIS));
        // Total visible width within columns
        assert!(display_width(&s) < 20); // accounting for margin
    }

    #[test]
    fn merge_drops_wrapped_when_budget_below_minimum() {
        // edgee very long, no room left for wrapped
        let s = merge_outputs(inputs(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJ",
            Some("WRAPPED"),
            " | ",
            Position::Left,
            40,
        ));
        // Only Edgee survives.
        assert_eq!(s, "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJ");
    }

    #[test]
    fn merge_never_truncates_edgee_even_when_overflow() {
        // Even if Edgee is wider than columns, it is emitted verbatim.
        let edgee = "X".repeat(50);
        let s = merge_outputs(inputs(&edgee, Some("WRAPPED"), " | ", Position::Left, 20));
        assert_eq!(s, edgee);
    }

    #[test]
    fn merge_with_ansi_in_edgee_keeps_wrapped_within_budget() {
        // ANSI-styled Edgee occupies 5 visible cells, leaving room for wrapped.
        let edgee = "\x1b[31mEDGEE\x1b[0m";
        let wrapped = "0123456789ABCDEF";
        let s = merge_outputs(inputs(edgee, Some(wrapped), " | ", Position::Left, 20));
        // Edgee's ANSI must survive verbatim
        assert!(s.contains("\x1b[31m"));
        // Display width budget respected (within margin)
        assert!(display_width(&s) < 20);
    }

    #[test]
    fn merge_with_wide_unicode_in_wrapped() {
        // Each 三 = 2 cells. columns=20, edgee=5, sep=3, margin=1 → budget=11
        // "三三三三三" = 10 cells → fits exactly.
        let s = merge_outputs(inputs(
            "EDGEE",
            Some("三三三三三"),
            " | ",
            Position::Left,
            20,
        ));
        assert!(s.contains("三三三三三"));
        assert!(display_width(&s) < 20);
    }

    #[test]
    fn merge_with_emoji_truncates_correctly() {
        // 🚀 = 2 cells
        let s = merge_outputs(inputs(
            "EDGEE",
            Some("🚀🚀🚀🚀🚀🚀🚀🚀"),
            " | ",
            Position::Left,
            20,
        ));
        assert!(display_width(&s) < 20);
    }

    #[test]
    fn merge_handles_zero_columns_gracefully() {
        // saturating_sub keeps budget at 0 → wrapped dropped; Edgee emitted.
        let s = merge_outputs(inputs("EDGEE", Some("WRAPPED"), " | ", Position::Left, 0));
        assert_eq!(s, "EDGEE");
    }

    #[test]
    fn trim_to_one_line_takes_first_line() {
        assert_eq!(trim_to_one_line("hello\nworld"), "hello");
        assert_eq!(trim_to_one_line(""), "");
        assert_eq!(trim_to_one_line("trailing  \n"), "trailing");
    }

    #[tokio::test]
    async fn run_wrapped_captures_stdout() {
        let out = run_wrapped("printf 'hi'".to_string(), Vec::new(), false)
            .await
            .unwrap();
        assert_eq!(out, "hi");
    }

    #[tokio::test]
    async fn run_wrapped_passes_stdin() {
        let out = run_wrapped("cat".to_string(), b"piped".to_vec(), false)
            .await
            .unwrap();
        assert_eq!(out, "piped");
    }

    #[tokio::test]
    async fn run_wrapped_errors_on_nonzero_exit() {
        let res = run_wrapped("exit 1".to_string(), Vec::new(), false).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn run_merge_falls_back_to_edgee_on_wrapped_failure() {
        unsafe {
            std::env::remove_var("EDGEE_SESSION_ID");
        }
        let line = run_merge("exit 1".to_string(), Vec::new()).await;
        assert!(line.contains("Edgee"));
        assert!(!line.contains(" │ "));
    }

    #[tokio::test]
    async fn run_merge_combines_when_both_succeed() {
        unsafe {
            std::env::remove_var("EDGEE_SESSION_ID");
            std::env::set_var("COLUMNS", "200");
            std::env::set_var("EDGEE_STATUSLINE_SEPARATOR", " | ");
        }
        let line = run_merge("printf OTHER".to_string(), Vec::new()).await;
        assert!(line.contains("Edgee"));
        assert!(line.contains("OTHER"));
        assert!(line.contains(" | "));
    }

    #[tokio::test]
    async fn run_merge_times_out_wrapped_command() {
        unsafe {
            std::env::remove_var("EDGEE_SESSION_ID");
            std::env::set_var("EDGEE_STATUSLINE_TIMEOUT_MS", "100");
        }
        let line = run_merge("sleep 2".to_string(), Vec::new()).await;
        // After timeout, Edgee should still render alone (no orphan separator).
        assert!(line.contains("Edgee"));
        unsafe {
            std::env::remove_var("EDGEE_STATUSLINE_TIMEOUT_MS");
        }
    }
}
