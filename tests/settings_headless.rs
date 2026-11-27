use std::path::Path;
use std::process::{Command, Output, Stdio};

fn edgee(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_edgee"))
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("EDGEE_NO_UPDATE_CHECK", "1")
        .current_dir(home)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn no_tty_and_no_agent_is_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    let out = edgee(home.path(), &["settings"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(2), "stderr: {err}");
    assert!(!err.contains("not a terminal"), "stderr: {err}");
    assert!(err.contains("AGENT"), "stderr: {err}");
}

#[test]
fn json_without_agent_is_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    let out = edgee(home.path(), &["settings", "--json"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
}

#[test]
fn unknown_technique_is_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    let out = edgee(home.path(), &["settings", "claude", "--set", "bogus=on"]);
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
}

#[test]
fn malformed_set_value_is_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    for bad in ["output-brevity", "output-brevity=maybe", "=on"] {
        let out = edgee(home.path(), &["settings", "claude", "--set", bad]);
        assert_eq!(out.status.code(), Some(2), "{bad}: {}", stderr(&out));
    }
}

#[test]
fn conflicting_sets_for_one_technique_are_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    let out = edgee(
        home.path(),
        &[
            "settings",
            "claude",
            "--set",
            "output-brevity=on",
            "--set",
            "output-brevity=off",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
}

#[test]
fn show_without_credentials_is_a_runtime_error_with_a_next_step() {
    let home = tempfile::tempdir().unwrap();
    let out = edgee(home.path(), &["settings", "claude", "--json"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "stderr: {err}");
    assert!(err.contains("edgee auth login"), "stderr: {err}");
    assert!(
        out.stdout.is_empty(),
        "stdout must stay clean for JSON consumers"
    );
}
