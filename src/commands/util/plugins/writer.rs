//! Reconciles a [`Plan`] onto disk.
//!
//! The tree under the plugin root is **exclusively Edgee-owned**, which is what
//! removes the need for a manifest: the desired set of files *is* the record of
//! what should exist, so anything else under the root is stale by definition and
//! can be swept. A plugin that stops being active, or a skill deleted from one,
//! disappears with no bookkeeping file to keep in step.
//!
//! Per-plugin the strategy is erase-and-rebuild rather than a file-level diff:
//! these are small text files, and wiping the directory removes the whole class
//! of bug where a renamed skill leaves its old directory behind.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::materialize::Plan;

/// What a reconcile actually did, for the caller to report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub written: usize,
    pub removed: usize,
    /// Files already byte-identical on disk. A steady-state launch is all skips.
    pub unchanged: usize,
}

/// Makes `root` exactly equal `plan`.
///
/// `guard` is the directory the root must live under — [`crate::config::edgee_home`]
/// in production. A path-construction bug must never be able to delete outside
/// Edgee's own directory, so this is checked before anything is removed.
pub fn reconcile(root: &Path, plan: &Plan, guard: &Path) -> Result<Stats> {
    if !root.starts_with(guard) {
        anyhow::bail!(
            "refusing to reconcile {} — outside {}",
            root.display(),
            guard.display()
        );
    }

    let mut stats = Stats::default();
    let desired: BTreeSet<PathBuf> = plan.files.iter().map(|f| f.path.clone()).collect();

    // Sweep first so a rename frees its old path before the new one is written.
    for existing in existing_files(root)? {
        let rel = existing
            .strip_prefix(root)
            .unwrap_or(&existing)
            .to_path_buf();
        if !desired.contains(&rel) {
            fs::remove_file(&existing)
                .with_context(|| format!("Failed to remove {}", existing.display()))?;
            stats.removed += 1;
        }
    }

    for file in &plan.files {
        let abs = root.join(&file.path);
        // Skipping an unchanged file keeps a steady-state launch free of writes,
        // and keeps mtimes stable for anything watching the tree.
        if fs::read_to_string(&abs).is_ok_and(|current| current == file.contents) {
            stats.unchanged += 1;
            continue;
        }
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        write_atomic(&abs, &file.contents)?;
        stats.written += 1;
    }

    prune_empty_dirs(root)?;
    Ok(stats)
}

/// Removes everything under `root`, subject to the same guard. Used when the
/// on-disk layout version no longer matches the one this CLI writes.
pub fn wipe(root: &Path, guard: &Path) -> Result<()> {
    if !root.starts_with(guard) {
        anyhow::bail!(
            "refusing to wipe {} — outside {}",
            root.display(),
            guard.display()
        );
    }
    if root.exists() {
        fs::remove_dir_all(root).with_context(|| format!("Failed to remove {}", root.display()))?;
    }
    Ok(())
}

/// Temp file plus rename, the house pattern (`claude_settings::write_settings`,
/// `session_log::store_session_log`). A crash mid-write leaves the previous file
/// intact rather than a truncated one.
fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("edgee-tmp");
    fs::write(&tmp, contents).with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Every regular file under `root`. Symlinks are reported but never followed —
/// traversing one could walk us out of the guarded directory.
fn existing_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Depth-first prune, so a directory emptied by removing its only child is
/// removed too. `root` itself is kept.
fn prune_empty_dirs(root: &Path) -> Result<()> {
    fn walk(dir: &Path, root: &Path) -> Result<()> {
        let entries: Vec<PathBuf> = match fs::read_dir(dir) {
            Ok(e) => e.flatten().map(|x| x.path()).collect(),
            Err(_) => return Ok(()),
        };
        for path in entries {
            if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                walk(&path, root)?;
            }
        }
        if dir != root && fs::read_dir(dir).is_ok_and(|mut e| e.next().is_none()) {
            let _ = fs::remove_dir(dir);
        }
        Ok(())
    }
    walk(root, root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::util::plugins::materialize::PlannedFile;

    fn plan(files: &[(&str, &str)]) -> Plan {
        Plan {
            files: files
                .iter()
                .map(|(p, c)| PlannedFile {
                    path: PathBuf::from(p),
                    contents: (*c).to_string(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// The root is a parameter, so these never touch `$HOME` and need no env
    /// lock — the same reason `claude_settings::discover_project` takes its
    /// start directory as an argument.
    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn read(root: &Path, rel: &str) -> Option<String> {
        fs::read_to_string(root.join(rel)).ok()
    }

    #[test]
    fn reconcile_creates_the_planned_tree() {
        let dir = root();
        let stats = reconcile(
            dir.path(),
            &plan(&[
                ("house/skills/a/SKILL.md", "alpha"),
                ("house/.mcp.json", "{}"),
            ]),
            dir.path(),
        )
        .unwrap();

        assert_eq!(stats.written, 2);
        assert_eq!(
            read(dir.path(), "house/skills/a/SKILL.md").as_deref(),
            Some("alpha")
        );
    }

    /// The sweep is what makes a manifest unnecessary — this is that claim.
    #[test]
    fn reconcile_removes_files_that_left_the_plan() {
        let dir = root();
        reconcile(
            dir.path(),
            &plan(&[
                ("house/skills/a/SKILL.md", "alpha"),
                ("house/skills/b/SKILL.md", "beta"),
            ]),
            dir.path(),
        )
        .unwrap();

        // `b` was deleted from the plugin in the console.
        let stats = reconcile(
            dir.path(),
            &plan(&[("house/skills/a/SKILL.md", "alpha")]),
            dir.path(),
        )
        .unwrap();

        assert_eq!(stats.removed, 1);
        assert!(read(dir.path(), "house/skills/b/SKILL.md").is_none());
        // …and the directory it lived in goes with it.
        assert!(!dir.path().join("house/skills/b").exists());
        assert_eq!(
            read(dir.path(), "house/skills/a/SKILL.md").as_deref(),
            Some("alpha")
        );
    }

    #[test]
    fn reconcile_rewrites_changed_content() {
        let dir = root();
        reconcile(
            dir.path(),
            &plan(&[("p/skills/a/SKILL.md", "v1")]),
            dir.path(),
        )
        .unwrap();

        let stats = reconcile(
            dir.path(),
            &plan(&[("p/skills/a/SKILL.md", "v2")]),
            dir.path(),
        )
        .unwrap();

        assert_eq!(stats.written, 1);
        assert_eq!(stats.unchanged, 0);
        assert_eq!(
            read(dir.path(), "p/skills/a/SKILL.md").as_deref(),
            Some("v2")
        );
    }

    /// A launch where nothing changed should do no filesystem work at all.
    #[test]
    fn reconcile_is_idempotent_and_skips_unchanged_files() {
        let dir = root();
        let p = plan(&[("p/skills/a/SKILL.md", "same")]);
        reconcile(dir.path(), &p, dir.path()).unwrap();

        let stats = reconcile(dir.path(), &p, dir.path()).unwrap();

        assert_eq!(stats.written, 0);
        assert_eq!(stats.removed, 0);
        assert_eq!(stats.unchanged, 1);
    }

    /// How an unassignment propagates: the API reports nothing active, the plan
    /// is empty, and the tree empties itself.
    #[test]
    fn reconcile_to_an_empty_plan_empties_the_root() {
        let dir = root();
        reconcile(
            dir.path(),
            &plan(&[("p/skills/a/SKILL.md", "x")]),
            dir.path(),
        )
        .unwrap();

        let stats = reconcile(dir.path(), &Plan::default(), dir.path()).unwrap();

        assert_eq!(stats.removed, 1);
        assert!(dir.path().exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// The blast radius of a path bug is deletion, so the guard is tested
    /// explicitly rather than trusted.
    #[test]
    fn reconcile_refuses_a_root_outside_the_guard() {
        let guard = root();
        let elsewhere = root();
        let marker = elsewhere.path().join("precious.txt");
        fs::write(&marker, "do not delete").unwrap();

        let err = reconcile(elsewhere.path(), &Plan::default(), guard.path()).unwrap_err();

        assert!(err.to_string().contains("refusing to reconcile"));
        assert_eq!(fs::read_to_string(&marker).unwrap(), "do not delete");
    }

    #[test]
    fn wipe_refuses_outside_the_guard_and_clears_inside_it() {
        let guard = root();
        let elsewhere = root();
        fs::write(elsewhere.path().join("keep.txt"), "keep").unwrap();
        assert!(wipe(elsewhere.path(), guard.path()).is_err());
        assert!(elsewhere.path().join("keep.txt").exists());

        let inner = guard.path().join("claude");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("x.md"), "x").unwrap();
        wipe(&inner, guard.path()).unwrap();
        assert!(!inner.exists());
    }

    /// A dangling symlink under the root must not abort the sweep, and must
    /// never be followed out of the guarded directory.
    #[cfg(unix)]
    #[test]
    fn reconcile_does_not_follow_symlinks_out_of_the_root() {
        let dir = root();
        let outside = root();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "secret").unwrap();

        fs::create_dir_all(dir.path().join("p")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("p/link")).unwrap();

        reconcile(dir.path(), &Plan::default(), dir.path()).unwrap();

        // The link itself is swept as a stale entry; its target is untouched.
        assert_eq!(fs::read_to_string(&secret).unwrap(), "secret");
    }
}
