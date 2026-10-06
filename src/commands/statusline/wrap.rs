//! `edgee statusline --wrap <command>`: Edgee's segment next to the output of a
//! statusLine of the user's own, so installing Edgee never costs them theirs.
//!
//! The wrapped command gets the same stdin, runs through the platform shell, and
//! is dropped if it fails or takes longer than [`TIMEOUT`]. Edgee's segment is
//! never truncated; theirs gets the width that is left.

use std::io::Read;
use std::process::Stdio;
use std::time::Duration;

use console::{measure_text_width, truncate_str};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::render;

const TIMEOUT: Duration = Duration::from_secs(2);
const SEPARATOR: &str = " │ ";
/// Below this many cells, their output is dropped rather than shown as a stub.
const MIN_WIDTH: usize = 10;

pub async fn run(command: &str) {
    let mut stdin = Vec::new();
    let _ = std::io::stdin().lock().read_to_end(&mut stdin);

    let (edgee, theirs) = tokio::join!(
        render::render(),
        tokio::time::timeout(TIMEOUT, run_theirs(command, stdin))
    );
    let columns = std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.trim().parse().ok())
        .filter(|&c| c > 0);
    let line = merge(&edgee, theirs.ok().flatten().as_deref(), columns);
    if !line.is_empty() {
        println!("{line}");
    }
}

/// Their stdout, or `None` when the command could not run or exited non-zero.
async fn run_theirs(command: &str, stdin: Vec<u8>) -> Option<String> {
    let mut child = shell(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(&stdin).await;
    }
    let output = child.wait_with_output().await.ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn shell(command: &str) -> Command {
    let (program, flag) = if cfg!(windows) {
        ("cmd.exe", "/C")
    } else {
        ("/bin/sh", "-c")
    };
    let mut cmd = Command::new(program);
    cmd.arg(flag).arg(command);
    cmd
}

/// One line: Edgee's segment, then the first line of theirs, fitted to `columns`
/// when the terminal width is known. Either side alone is returned as is.
fn merge(edgee: &str, theirs: Option<&str>, columns: Option<usize>) -> String {
    let theirs = theirs
        .and_then(|t| t.lines().next())
        .map(str::trim_end)
        .filter(|t| !t.is_empty());
    let Some(theirs) = theirs else {
        return edgee.to_string();
    };
    if edgee.is_empty() {
        return theirs.to_string();
    }

    let theirs = match columns {
        // One spare cell, so the line never wraps right at the edge.
        Some(columns) => {
            let used = measure_text_width(edgee) + measure_text_width(SEPARATOR) + 1;
            let budget = columns.saturating_sub(used);
            if budget < MIN_WIDTH {
                return edgee.to_string();
            }
            truncate_str(theirs, budget, "…").into_owned()
        }
        None => theirs.to_string(),
    };
    format!("{edgee}{SEPARATOR}{theirs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn either_side_alone_is_passed_through() {
        assert_eq!(merge("E", None, None), "E");
        assert_eq!(merge("E", Some(" \n"), None), "E");
        assert_eq!(merge("", Some("theirs"), None), "theirs");
        assert_eq!(merge("", None, None), "");
    }

    #[test]
    fn both_sides_share_one_line() {
        assert_eq!(merge("E", Some("a\nb\n"), None), "E │ a");
    }

    #[test]
    fn theirs_is_truncated_to_the_remaining_width() {
        // 1 (edgee) + 3 (separator) + 1 (margin) leaves 15 of 20 cells.
        let merged = merge("E", Some("0123456789abcdefghij"), Some(20));
        assert_eq!(merged, "E │ 0123456789abcd…");
        assert_eq!(measure_text_width(&merged), 19);
    }

    #[test]
    fn theirs_is_dropped_when_too_little_room_is_left() {
        assert_eq!(merge("E", Some("0123456789"), Some(12)), "E");
    }

    #[test]
    fn truncation_ignores_ansi_codes() {
        let merged = merge("E", Some("\x1b[31mred text here\x1b[0m"), Some(16));
        assert!(measure_text_width(&merged) <= 15, "{merged:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn their_command_receives_stdin() {
        let out = run_theirs("cat", b"hello".to_vec()).await;
        assert_eq!(out.as_deref(), Some("hello"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_command_yields_nothing() {
        assert_eq!(run_theirs("echo hi; exit 3", Vec::new()).await, None);
    }
}
