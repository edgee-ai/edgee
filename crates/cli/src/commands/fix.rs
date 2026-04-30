//! `edgee fix` — overlay Edgee's statusline on top of a conflicting
//! project-level `statusLine` command, by writing into
//! `.claude/settings.local.json` (never the shared `.claude/settings.json`).

use anyhow::{Context, Result};
use console::style;

use crate::commands::claude_settings::{self, CommandKind, LOCAL_FILE};
use crate::commands::doctor::{self, ConflictStatus, Diagnosis};

setup_command! {
    /// Apply the fix without prompting for confirmation.
    #[arg(long)]
    pub yes: bool,
}

pub async fn run(_opts: Options) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let diag = doctor::diagnose(&cwd)?;

    match diag.status {
        ConflictStatus::None => {
            println!(
                "  {} No conflict detected — nothing to do.",
                style("✓").green(),
            );
            Ok(())
        }
        ConflictStatus::Wrapped => {
            println!(
                "  {} Edgee overlay already in place — nothing to do.",
                style("✓").green(),
            );
            Ok(())
        }
        ConflictStatus::Shadowed => apply(&diag),
    }
}

fn apply(diag: &Diagnosis) -> Result<()> {
    if std::env::var_os("EDGEE_NO_AUTO_OVERLAY").is_some() {
        println!(
            "  {} EDGEE_NO_AUTO_OVERLAY is set — refusing to write. Manual overlay required.",
            style("⚠").yellow(),
        );
        if let Some(cmd) = &diag.effective_command {
            println!();
            println!("  Suggested manual overlay (paste into .claude/settings.local.json):");
            println!();
            println!(
                "    \"statusLine\": {{ \"type\": \"command\", \"command\": \"edgee statusline --wrap '{}'\" }}",
                claude_settings::posix_single_quote_escape(cmd)
            );
            println!();
        }
        return Ok(());
    }

    let project_root = diag
        .project_root
        .as_ref()
        .context("internal: SHADOWED status without a project root")?;
    let original = diag
        .effective_command
        .as_deref()
        .context("internal: SHADOWED status without an effective command")?;

    // Self-wrap protection (defense in depth — diagnose() already classifies
    // existing wraps as WRAPPED, but be safe in case classification drifts).
    if matches!(
        claude_settings::classify_command(original),
        CommandKind::EdgeeWrap
    ) {
        anyhow::bail!(
            "Refusing to wrap a command that already starts with `edgee statusline --wrap`."
        );
    }

    let local_path = project_root.join(".claude").join(LOCAL_FILE);
    let mut local_value = if local_path.is_file() {
        claude_settings::read_settings(&local_path)?.value
    } else {
        serde_json::Value::Object(Default::default())
    };

    let escaped = claude_settings::posix_single_quote_escape(original);
    let new_command = format!("edgee statusline --wrap '{escaped}'");
    claude_settings::set_status_line(&mut local_value, &new_command, Some(10));

    claude_settings::write_settings(&local_path, &local_value)?;

    println!(
        "  {} Wrote overlay to {}",
        style("✓").green(),
        style(local_path.display()).cyan()
    );
    println!(
        "  {} Wrapping: {}",
        style("→").dim(),
        style(original).dim()
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    /// Tests in this module mutate process-global env vars and must run
    /// serially with every other module that does the same.
    use crate::commands::claude_settings::env_test_lock as env_lock;

    fn write_json(path: &std::path::Path, value: serde_json::Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn isolate_home(home: &PathBuf) -> impl Drop {
        let prev = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home);
        }
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(prev) => unsafe { std::env::set_var("HOME", prev) },
                    None => unsafe { std::env::remove_var("HOME") },
                }
            }
        }
        Restore(prev)
    }

    fn isolate_var<F: FnOnce()>(key: &str, value: Option<&str>, f: F) {
        let prev = std::env::var_os(key);
        match value {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
        f();
        match prev {
            Some(p) => unsafe { std::env::set_var(key, p) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    fn fixture(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(format!("home-{name}"));
        let proj = tmp.path().join(format!("proj-{name}"));
        fs::create_dir_all(&proj).unwrap();
        (tmp, home, proj)
    }

    fn read(path: &std::path::Path) -> serde_json::Value {
        let s = fs::read_to_string(path).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn fix_writes_overlay_when_shared_shadows() {
        let (_tmp, home, proj) = fixture("shadow-shared");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"command": "/path/to/tool.sh"}}),
        );
        let _lock = env_lock();
        let _h = isolate_home(&home);
        isolate_var("EDGEE_NO_AUTO_OVERLAY", None, || {
            let diag = doctor::diagnose(&proj).unwrap();
            apply(&diag).unwrap();
        });

        let local_path = proj.join(".claude").join("settings.local.json");
        assert!(local_path.is_file());
        let v = read(&local_path);
        assert_eq!(
            v["statusLine"]["command"],
            "edgee statusline --wrap '/path/to/tool.sh'"
        );
        assert_eq!(v["statusLine"]["type"], "command");
    }

    #[test]
    fn fix_preserves_unrelated_keys_in_local() {
        let (_tmp, home, proj) = fixture("merge-preserve");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"command": "/path/to/tool.sh"}}),
        );
        write_json(
            &proj.join(".claude").join("settings.local.json"),
            json!({"theme": "dark", "env": {"FOO": "bar"}}),
        );
        let _lock = env_lock();
        let _h = isolate_home(&home);
        isolate_var("EDGEE_NO_AUTO_OVERLAY", None, || {
            let diag = doctor::diagnose(&proj).unwrap();
            apply(&diag).unwrap();
        });

        let v = read(&proj.join(".claude").join(LOCAL_FILE));
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["env"]["FOO"], "bar");
        assert_eq!(
            v["statusLine"]["command"],
            "edgee statusline --wrap '/path/to/tool.sh'"
        );
    }

    #[test]
    fn fix_never_writes_to_shared_settings() {
        let (_tmp, home, proj) = fixture("never-shared");
        let shared = proj.join(".claude").join("settings.json");
        write_json(&shared, json!({"statusLine": {"command": "/x.sh"}}));
        let shared_before = fs::read_to_string(&shared).unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        isolate_var("EDGEE_NO_AUTO_OVERLAY", None, || {
            let diag = doctor::diagnose(&proj).unwrap();
            apply(&diag).unwrap();
        });

        let shared_after = fs::read_to_string(&shared).unwrap();
        assert_eq!(shared_before, shared_after, "shared file must be untouched");
    }

    #[test]
    fn fix_escapes_single_quotes_in_original_command() {
        let (_tmp, home, proj) = fixture("escape-single-quote");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"command": "echo \"it's working\""}}),
        );
        let _lock = env_lock();
        let _h = isolate_home(&home);
        isolate_var("EDGEE_NO_AUTO_OVERLAY", None, || {
            let diag = doctor::diagnose(&proj).unwrap();
            apply(&diag).unwrap();
        });

        let v = read(&proj.join(".claude").join(LOCAL_FILE));
        let cmd = v["statusLine"]["command"].as_str().unwrap();
        // The escaped form must be wrapped in single quotes with `'\''` for
        // the embedded single quote.
        assert!(cmd.starts_with("edgee statusline --wrap '"));
        assert!(cmd.ends_with("'"));
        assert!(cmd.contains("it'\\''s working"));
    }

    #[test]
    fn fix_escapes_special_chars_via_single_quotes() {
        // Backticks, $, backslashes survive POSIX single-quote escaping.
        let (_tmp, home, proj) = fixture("escape-special");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"command": "echo `whoami` $HOME \\ done"}}),
        );
        let _lock = env_lock();
        let _h = isolate_home(&home);
        isolate_var("EDGEE_NO_AUTO_OVERLAY", None, || {
            let diag = doctor::diagnose(&proj).unwrap();
            apply(&diag).unwrap();
        });

        let v = read(&proj.join(".claude").join(LOCAL_FILE));
        let cmd = v["statusLine"]["command"].as_str().unwrap();
        assert_eq!(
            cmd,
            "edgee statusline --wrap 'echo `whoami` $HOME \\ done'"
        );
    }

    #[test]
    fn fix_idempotent_when_already_wrapped() {
        let (_tmp, home, proj) = fixture("idempotent");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"command": "/path/to/tool.sh"}}),
        );

        let _lock = env_lock();
        let _h = isolate_home(&home);
        isolate_var("EDGEE_NO_AUTO_OVERLAY", None, || {
            let diag = doctor::diagnose(&proj).unwrap();
            apply(&diag).unwrap();
        });
        let after_first = read(&proj.join(".claude").join(LOCAL_FILE));

        // Second run: now status is WRAPPED; apply() shouldn't be invoked,
        // but we exercise it anyway and confirm the file is unchanged.
        isolate_var("EDGEE_NO_AUTO_OVERLAY", None, || {
            let diag = doctor::diagnose(&proj).unwrap();
            assert_eq!(diag.status, ConflictStatus::Wrapped);
        });
        let after_second = read(&proj.join(".claude").join(LOCAL_FILE));
        assert_eq!(after_first, after_second);
    }

    /// End-to-end realistic shadowing scenario: a project ships a shared
    /// `.claude/settings.json` whose statusLine prints `HELLO_FROM_OTHER_TOOL`.
    /// We:
    ///   1. diagnose → SHADOWED.
    ///   2. apply the fix → write `.claude/settings.local.json` with our wrap.
    ///   3. read the resulting wrap command back.
    ///   4. extract the wrapped command (between single quotes).
    ///   5. run the wrap merge with that command and verify the output
    ///      preserves Edgee verbatim AND contains `HELLO_FROM_OTHER_TOOL`.
    #[tokio::test]
    async fn end_to_end_shadow_then_fix_then_wrap_renders_both() {
        let (_tmp, home, proj) = fixture("e2e");
        let other_tool_marker = "HELLO_FROM_OTHER_TOOL";
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {
                "type": "command",
                "command": format!("echo {other_tool_marker}")
            }}),
        );

        let _lock = env_lock();
        let _h = isolate_home(&home);

        // Make sure unrelated env vars don't leak between tests.
        unsafe {
            std::env::remove_var("EDGEE_NO_AUTO_OVERLAY");
            std::env::remove_var("EDGEE_SESSION_ID");
            std::env::remove_var("EDGEE_HAS_EXISTING_STATUSLINE");
            std::env::remove_var("EDGEE_STATUSLINE_TIMEOUT_MS");
            std::env::set_var("COLUMNS", "200");
            std::env::set_var("EDGEE_STATUSLINE_SEPARATOR", " | ");
            std::env::remove_var("EDGEE_STATUSLINE_POSITION");
        }

        // 1 + 2: diagnose & apply.
        let diag = doctor::diagnose(&proj).unwrap();
        assert_eq!(diag.status, ConflictStatus::Shadowed);
        apply(&diag).unwrap();

        // 3: read the resulting wrap command.
        let local = read(&proj.join(".claude").join(LOCAL_FILE));
        let wrap_cmd = local["statusLine"]["command"].as_str().unwrap();
        assert!(wrap_cmd.starts_with("edgee statusline --wrap '"));
        assert!(wrap_cmd.ends_with("'"));

        // 4: extract the wrapped command (everything between the first `'`
        // and the last `'`, with POSIX `'\''` escapes unwrapped).
        let inner_quoted = &wrap_cmd["edgee statusline --wrap '".len()..wrap_cmd.len() - 1];
        let inner = inner_quoted.replace("'\\''", "'");
        assert_eq!(inner, format!("echo {other_tool_marker}"));

        // 5: run the wrap merge with that command. We bypass the binary and
        // call the merge function directly so the test stays hermetic.
        let merged = crate::commands::statusline::wrap::run_merge_for_test(inner).await;

        assert!(
            merged.contains("Edgee"),
            "Edgee segment must appear: {merged:?}"
        );
        assert!(
            merged.contains(other_tool_marker),
            "wrapped marker must appear: {merged:?}"
        );
        assert!(
            merged.contains(" | "),
            "separator must appear when both render: {merged:?}"
        );

        unsafe {
            std::env::remove_var("COLUMNS");
            std::env::remove_var("EDGEE_STATUSLINE_SEPARATOR");
        }
    }

    #[test]
    fn fix_refuses_when_no_auto_overlay_is_set() {
        let (_tmp, home, proj) = fixture("no-auto");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"command": "/path/to/tool.sh"}}),
        );
        let _lock = env_lock();
        let _h = isolate_home(&home);
        isolate_var("EDGEE_NO_AUTO_OVERLAY", Some("1"), || {
            let diag = doctor::diagnose(&proj).unwrap();
            apply(&diag).unwrap();
        });

        // Local file must not have been written.
        assert!(!proj.join(".claude").join(LOCAL_FILE).is_file());
    }
}
