//! `edgee doctor` — diagnose Claude Code statusLine conflicts in the
//! current project.
//!
//! Read-only. Walks up from the current working directory looking for
//! `.claude/settings.json` and `.claude/settings.local.json`, computes the
//! effective `statusLine` command using Claude Code's precedence (local >
//! shared > user), classifies the situation, and prints a human or JSON
//! report.

use anyhow::Result;
use console::style;
use serde::Serialize;
use serde_json::json;

use crate::commands::claude_settings::{self, CommandKind, StatusLineSource};

setup_command! {
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ConflictStatus {
    /// No project-level statusLine, or it is already plain Edgee.
    None,
    /// Project-level statusLine is already an `edgee statusline --wrap`
    /// overlay — coexistence is in place.
    Wrapped,
    /// Project-level statusLine shadows Edgee's user-level statusline.
    Shadowed,
}

impl ConflictStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Wrapped => "WRAPPED",
            Self::Shadowed => "SHADOWED",
        }
    }
}

#[derive(Debug)]
pub struct Diagnosis {
    pub project_root: Option<std::path::PathBuf>,
    pub effective_command: Option<String>,
    pub effective_source: Option<StatusLineSource>,
    pub command_kind: CommandKind,
    pub status: ConflictStatus,
    pub user_has_edgee: bool,
}

impl Diagnosis {
    pub fn suggestion(&self) -> Option<String> {
        match self.status {
            ConflictStatus::None => None,
            ConflictStatus::Wrapped => None,
            ConflictStatus::Shadowed => {
                if std::env::var_os("EDGEE_NO_AUTO_OVERLAY").is_some() {
                    Some(
                        "EDGEE_NO_AUTO_OVERLAY is set; manual overlay required. \
                         Unset it and run `edgee fix`."
                            .to_string(),
                    )
                } else {
                    Some(
                        "Run `edgee fix` to overlay Edgee's statusline on top of \
                         the project's command (writes .claude/settings.local.json)."
                            .to_string(),
                    )
                }
            }
        }
    }
}

pub async fn run(opts: Options) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let diag = diagnose(&cwd)?;

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&diag.to_json())?);
    } else {
        print_human(&diag);
    }
    Ok(())
}

pub fn diagnose(cwd: &std::path::Path) -> Result<Diagnosis> {
    let project = claude_settings::discover_project(cwd)?;
    let user = claude_settings::read_user_settings()?;

    let user_has_edgee = user
        .as_ref()
        .and_then(|f| f.value.get("statusLine"))
        .and_then(claude_settings::status_line_command)
        .map(claude_settings::classify_command)
        .map(|k| matches!(k, CommandKind::Edgee | CommandKind::EdgeeWrap))
        .unwrap_or(false);

    let effective = claude_settings::effective_status_line(&project, user.as_ref());
    let (effective_source, effective_command) = match effective {
        Some((src, sl)) => (
            Some(src),
            claude_settings::status_line_command(sl).map(str::to_string),
        ),
        None => (None, None),
    };

    let command_kind = effective_command
        .as_deref()
        .map(claude_settings::classify_command)
        .unwrap_or(CommandKind::Absent);

    let status = classify_status(effective_source, command_kind);

    Ok(Diagnosis {
        project_root: project.project_root.clone(),
        effective_command,
        effective_source,
        command_kind,
        status,
        user_has_edgee,
    })
}

fn classify_status(source: Option<StatusLineSource>, kind: CommandKind) -> ConflictStatus {
    let project_level = matches!(
        source,
        Some(StatusLineSource::ProjectShared) | Some(StatusLineSource::ProjectLocal)
    );
    match (project_level, kind) {
        (false, _) => ConflictStatus::None,
        (true, CommandKind::EdgeeWrap) => ConflictStatus::Wrapped,
        (true, CommandKind::Edgee) => ConflictStatus::None,
        (true, CommandKind::Absent) => ConflictStatus::None,
        (true, CommandKind::ThirdParty) => ConflictStatus::Shadowed,
    }
}

impl Diagnosis {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "status": self.status.as_str(),
            "project_root": self.project_root.as_deref().map(|p| p.to_string_lossy().into_owned()),
            "effective_command": self.effective_command,
            "effective_source": self.effective_source.map(|s| match s {
                StatusLineSource::User => "user",
                StatusLineSource::ProjectShared => "project_shared",
                StatusLineSource::ProjectLocal => "project_local",
            }),
            "command_kind": match self.command_kind {
                CommandKind::Absent => "absent",
                CommandKind::Edgee => "edgee",
                CommandKind::EdgeeWrap => "edgee_wrap",
                CommandKind::ThirdParty => "third_party",
            },
            "user_has_edgee": self.user_has_edgee,
            "suggestion": self.suggestion(),
        })
    }
}

fn print_human(diag: &Diagnosis) {
    println!();
    println!("  {}", style("Edgee doctor").bold());
    println!();
    match &diag.project_root {
        Some(p) => println!("  {} {}", style("Project").bold().underlined(), style(p.display()).cyan()),
        None => println!("  {} {}", style("Project").bold().underlined(), style("(no .claude config found)").dim()),
    }
    let source = diag
        .effective_source
        .map(|s| match s {
            StatusLineSource::User => "user level (~/.claude/settings.json)",
            StatusLineSource::ProjectShared => "project shared (.claude/settings.json)",
            StatusLineSource::ProjectLocal => "project local (.claude/settings.local.json)",
        })
        .unwrap_or("(no statusLine configured)");
    println!(
        "  {} {}",
        style("Effective source").bold().underlined(),
        style(source).cyan()
    );
    let cmd_disp = diag
        .effective_command
        .clone()
        .unwrap_or_else(|| "(none)".to_string());
    println!(
        "  {} {}",
        style("Effective command").bold().underlined(),
        style(&cmd_disp).cyan()
    );

    let status_styled = match diag.status {
        ConflictStatus::None => style("NONE").green().bold(),
        ConflictStatus::Wrapped => style("WRAPPED").green().bold(),
        ConflictStatus::Shadowed => style("SHADOWED").red().bold(),
    };
    println!("  {} {}", style("Conflict").bold().underlined(), status_styled);

    if let Some(suggestion) = diag.suggestion() {
        println!();
        println!("  {} {}", style("→").yellow(), suggestion);
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::claude_settings::env_test_lock as env_lock;
    use std::fs;
    use std::path::PathBuf;

    fn write_json(path: &std::path::Path, value: serde_json::Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn isolated_home(home: &PathBuf) -> impl Drop {
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

    fn make_project(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        // Use a unique HOME inside tmp so user-level reads don't escape the
        // sandbox. Caller picks up `tmp` and the cwd path.
        let home = tmp.path().join(format!("home-{name}"));
        let proj = tmp.path().join(format!("proj-{name}"));
        fs::create_dir_all(&proj).unwrap();
        (tmp, home, proj)
    }

    #[test]
    fn none_when_no_claude_anywhere() {
        let (_tmp, home, proj) = make_project("none");
        let _lock = env_lock();
        let _restore = isolated_home(&home);
        let diag = diagnose(&proj).unwrap();
        assert_eq!(diag.status, ConflictStatus::None);
        assert!(diag.project_root.is_none());
        assert!(diag.effective_command.is_none());
    }

    #[test]
    fn none_when_user_has_edgee_no_project_settings() {
        let (_tmp, home, proj) = make_project("user-only");
        write_json(
            &home.join(".claude").join("settings.json"),
            json!({"statusLine": {"type": "command", "command": "edgee statusline"}}),
        );
        let _lock = env_lock();
        let _restore = isolated_home(&home);
        let diag = diagnose(&proj).unwrap();
        assert_eq!(diag.status, ConflictStatus::None);
        assert!(diag.user_has_edgee);
        assert_eq!(diag.command_kind, CommandKind::Edgee);
    }

    #[test]
    fn shadowed_when_project_shared_has_third_party_command() {
        let (_tmp, home, proj) = make_project("shadow-shared");
        write_json(
            &home.join(".claude").join("settings.json"),
            json!({"statusLine": {"type": "command", "command": "edgee statusline"}}),
        );
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"type": "command", "command": "/path/to/other-tool.sh"}}),
        );
        let _lock = env_lock();
        let _restore = isolated_home(&home);
        let diag = diagnose(&proj).unwrap();
        assert_eq!(diag.status, ConflictStatus::Shadowed);
        assert_eq!(
            diag.effective_command.as_deref(),
            Some("/path/to/other-tool.sh")
        );
        assert_eq!(diag.command_kind, CommandKind::ThirdParty);
    }

    #[test]
    fn shadowed_when_project_local_has_third_party_command() {
        let (_tmp, home, proj) = make_project("shadow-local");
        write_json(
            &proj.join(".claude").join("settings.local.json"),
            json!({"statusLine": {"type": "command", "command": "/path/to/other-tool.sh"}}),
        );
        let _lock = env_lock();
        let _restore = isolated_home(&home);
        let diag = diagnose(&proj).unwrap();
        assert_eq!(diag.status, ConflictStatus::Shadowed);
    }

    #[test]
    fn wrapped_when_project_local_already_overlays() {
        let (_tmp, home, proj) = make_project("wrapped");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"type": "command", "command": "/path/to/other-tool.sh"}}),
        );
        write_json(
            &proj.join(".claude").join("settings.local.json"),
            json!({"statusLine": {
                "type": "command",
                "command": "edgee statusline --wrap '/path/to/other-tool.sh'"
            }}),
        );
        let _lock = env_lock();
        let _restore = isolated_home(&home);
        let diag = diagnose(&proj).unwrap();
        assert_eq!(diag.status, ConflictStatus::Wrapped);
        assert_eq!(diag.command_kind, CommandKind::EdgeeWrap);
    }

    #[test]
    fn none_when_project_already_uses_plain_edgee() {
        // If a project explicitly committed `edgee statusline` (no wrap),
        // there's no shadowing — Edgee runs.
        let (_tmp, home, proj) = make_project("project-edgee");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"type": "command", "command": "edgee statusline"}}),
        );
        let _lock = env_lock();
        let _restore = isolated_home(&home);
        let diag = diagnose(&proj).unwrap();
        assert_eq!(diag.status, ConflictStatus::None);
    }

    #[test]
    fn project_local_takes_precedence_over_shared() {
        let (_tmp, home, proj) = make_project("local-wins");
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"command": "shared-cmd"}}),
        );
        write_json(
            &proj.join(".claude").join("settings.local.json"),
            json!({"statusLine": {"command": "local-cmd"}}),
        );
        let _lock = env_lock();
        let _restore = isolated_home(&home);
        let diag = diagnose(&proj).unwrap();
        assert_eq!(diag.effective_command.as_deref(), Some("local-cmd"));
        assert_eq!(diag.effective_source, Some(StatusLineSource::ProjectLocal));
    }

    #[test]
    fn nested_cwd_finds_parent_project() {
        let (_tmp, home, proj) = make_project("nested");
        let deep = proj.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        write_json(
            &proj.join(".claude").join("settings.json"),
            json!({"statusLine": {"command": "/path/to/other.sh"}}),
        );
        let _lock = env_lock();
        let _restore = isolated_home(&home);
        let diag = diagnose(&deep).unwrap();
        assert_eq!(diag.project_root.as_deref(), Some(proj.as_path()));
        assert_eq!(diag.status, ConflictStatus::Shadowed);
    }
}
