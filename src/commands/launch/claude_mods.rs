//! Claude Code mods bundled with the CLI.
//!
//! A mod is a Claude Code plugin whose behaviour lives in a TypeScript module
//! (`hooks/hooks.json` → `modules`) that runs inside the session. Their sources
//! live in `mods/` at the repository root and are compiled into the binary, so
//! a launch always ships the version this CLI was tested against.
//!
//! Same rule as org plugin delivery: Edgee writes into a directory it owns
//! (`~/.edgee/mods/<name>`) and points Claude Code at it with `--plugin-dir`.
//! Nothing is written into the user's own `~/.claude`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The first Claude Code release that loads mods.
const MIN_CLAUDE_VERSION: (u64, u64, u64) = (2, 1, 287);

struct Mod {
    name: &'static str,
    /// Paths relative to the mod's directory, and their contents.
    files: &'static [(&'static str, &'static str)],
}

/// `/edgee-model`: pick the model serving the session through the session
/// reroute tools of the Edgee MCP server, and a pane of the session's requests.
/// Needs the Edgee MCP server and `EDGEE_SESSION_ID`.
const EDGEE_MODEL: Mod = Mod {
    name: "edgee-model",
    files: &[
        (
            ".claude-plugin/plugin.json",
            include_str!("../../../mods/edgee-model/.claude-plugin/plugin.json"),
        ),
        (
            "hooks/hooks.json",
            include_str!("../../../mods/edgee-model/hooks/hooks.json"),
        ),
        (
            "hooks/edgee-model.ts",
            include_str!("../../../mods/edgee-model/hooks/edgee-model.ts"),
        ),
        (
            "types/index.d.ts",
            include_str!("../../../mods/edgee-model/types/index.d.ts"),
        ),
    ],
};

/// The tools `edgee-model` calls on the Edgee MCP server, pre-allowed so the
/// pane's first pick does not stop on a permission prompt.
pub(super) const EDGEE_MODEL_TOOLS: &str =
    "mcp__edgee__listSessionModels,mcp__edgee__setSessionReroute,mcp__edgee__clearSessionReroute";

/// Writes the bundled mods and returns their directories for `--plugin-dir`.
///
/// Best-effort, like org plugin delivery: an opted-out user
/// (`EDGEE_MODS_DISABLED`), a Claude Code too old to load mods, or a write
/// failure yields no directory, and Claude starts without the mods.
pub(super) fn prepare(claude: &OsStr) -> Vec<PathBuf> {
    if crate::config::mods_disabled_env_override() == Some(true) {
        return Vec::new();
    }
    if !detect_mod_support(claude) {
        return Vec::new();
    }
    let Some(root) = crate::config::edgee_home().map(|home| home.join("mods")) else {
        return Vec::new();
    };
    match write_mod(&root, &EDGEE_MODEL) {
        Ok(dir) => vec![dir],
        Err(e) => {
            eprintln!(
                "{} {e:#}",
                console::style("Edgee mods not loaded:").yellow()
            );
            Vec::new()
        }
    }
}

fn detect_mod_support(claude: &OsStr) -> bool {
    std::process::Command::new(claude)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| parse_version(&String::from_utf8_lossy(&out.stdout)))
        .is_some_and(|version| version >= MIN_CLAUDE_VERSION)
}

/// `2.1.287 (Claude Code)` → `(2, 1, 287)`; a pre-release suffix is ignored.
fn parse_version(output: &str) -> Option<(u64, u64, u64)> {
    let mut parts = output.split_whitespace().next()?.split(['.', '-']);
    let mut next = || parts.next()?.parse::<u64>().ok();
    Some((next()?, next()?, next()?))
}

/// Writes `m` under `root/<name>`, touching only files whose contents changed:
/// Claude Code hot-reloads a mod when its folder changes, so rewriting
/// identical files would reload the mod in every session already running.
fn write_mod(root: &Path, m: &Mod) -> Result<PathBuf> {
    let dir = root.join(m.name);
    for (relative, contents) in m.files {
        let path = dir.join(relative);
        if std::fs::read_to_string(&path).is_ok_and(|current| current == *contents) {
            continue;
        }
        let parent = path.parent().context("mod file has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        // Write then rename, so a concurrent launch never reads half a file.
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_code_versions() {
        assert_eq!(parse_version("2.1.287 (Claude Code)\n"), Some((2, 1, 287)));
        assert_eq!(parse_version("2.2.0-beta.1 (Claude Code)"), Some((2, 2, 0)));
        assert_eq!(parse_version("not a version"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn mods_need_a_recent_claude_code() {
        let supports = |out: &str| parse_version(out).is_some_and(|v| v >= MIN_CLAUDE_VERSION);
        assert!(supports("2.1.287 (Claude Code)"));
        assert!(supports("2.10.0 (Claude Code)"));
        assert!(supports("3.0.0 (Claude Code)"));
        assert!(!supports("2.1.286 (Claude Code)"));
        assert!(!supports("1.99.999 (Claude Code)"));
    }

    #[test]
    fn writes_the_mod_and_leaves_unchanged_files_alone() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_mod(root.path(), &EDGEE_MODEL).unwrap();
        for (relative, contents) in EDGEE_MODEL.files {
            assert_eq!(&std::fs::read_to_string(dir.join(relative)).unwrap(), contents);
        }

        // A second launch must not touch the files, or running sessions would hot-reload.
        let module = dir.join("hooks/edgee-model.ts");
        let before = std::fs::metadata(&module).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_mod(root.path(), &EDGEE_MODEL).unwrap();
        assert_eq!(std::fs::metadata(&module).unwrap().modified().unwrap(), before);

        // A stale file (an older CLI's mod) is replaced.
        std::fs::write(&module, "stale").unwrap();
        write_mod(root.path(), &EDGEE_MODEL).unwrap();
        assert_ne!(std::fs::read_to_string(&module).unwrap(), "stale");
        assert!(!std::fs::read_dir(dir.join("hooks"))
            .unwrap()
            .any(|e| e.unwrap().file_name().to_string_lossy().contains(".tmp-")));
    }

    #[test]
    fn the_bundle_lists_the_module_hooks_json_names() {
        let hooks = EDGEE_MODEL
            .files
            .iter()
            .find(|(path, _)| *path == "hooks/hooks.json")
            .unwrap()
            .1;
        let json: serde_json::Value = serde_json::from_str(hooks).unwrap();
        for module in json["modules"].as_array().unwrap() {
            let path = format!("hooks/{}", module.as_str().unwrap().trim_start_matches("./"));
            assert!(
                EDGEE_MODEL.files.iter().any(|(p, _)| *p == path),
                "{path} is not bundled"
            );
        }
    }
}
