//! Mirrors a user's config directory into an Edgee-owned one using symlinks.
//!
//! Some agents have no way to point at an extra skills directory — they only let
//! you relocate the whole config root (Codex's `CODEX_HOME`). Copying that root
//! would duplicate the user's **credentials** (`auth.json`), history and
//! databases into a temp directory with different permissions, which is not a
//! trade worth making for plugin delivery.
//!
//! So the mirror is symlinks: every entry points back at the original, and only
//! the directories we add are real. The agent sees one coherent config root, the
//! user's files are never copied or modified, and deleting the mirror costs
//! nothing.
//!
//! Verified end to end against Codex 0.144.6: with `CODEX_HOME` pointed at a
//! mirror built this way, `auth.json` authenticated through the symlink and a
//! skill placed in the mirror's `skills/` was loaded — while the same prompt
//! against the real config root did not know it.
//!
//! Unix only. Windows symlinks need elevated privileges or developer mode, so
//! there the mirror is refused and the caller reports the kinds as undelivered
//! rather than falling back to copying credentials.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// Builds `dest` as a symlink mirror of `source`.
///
/// `merge_dirs` names directories that must stay writable — they are recreated
/// as real directories whose children are symlinked individually, so we can add
/// our own entries alongside the user's without shadowing theirs.
///
/// `dest` is rebuilt from scratch each call. It contains nothing but symlinks
/// and empty directories, so this is cheap and leaves no stale state.
#[cfg(unix)]
pub fn build(source: &Path, dest: &Path, merge_dirs: &[&str]) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest).with_context(|| format!("Failed to clear {}", dest.display()))?;
    }
    fs::create_dir_all(dest).with_context(|| format!("Failed to create {}", dest.display()))?;

    // Created up front, not after the loop: a user who has never run the agent
    // has no source directory at all, and must still get somewhere for our
    // additions to be linked into.
    for dir in merge_dirs {
        let _ = fs::create_dir_all(dest.join(dir));
    }

    // A missing source is not an error — see above.
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let from = entry.path();
        let to = dest.join(&name);
        let is_merge = name.to_str().is_some_and(|n| merge_dirs.contains(&n));

        if is_merge && fs::symlink_metadata(&from).is_ok_and(|m| m.is_dir()) {
            // Real directory, children symlinked — so we can add siblings.
            fs::create_dir_all(&to)?;
            if let Ok(children) = fs::read_dir(&from) {
                for child in children.flatten() {
                    let _ = std::os::unix::fs::symlink(child.path(), to.join(child.file_name()));
                }
            }
        } else {
            // Errors are ignored per-entry: one unreadable file must not stop the
            // agent from launching.
            let _ = std::os::unix::fs::symlink(&from, &to);
        }
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn build(_source: &Path, _dest: &Path, _merge_dirs: &[&str]) -> Result<()> {
    anyhow::bail!("config mirroring requires symlink support")
}

/// Links every immediate child of `from` into `into`, replacing any link of the
/// same name. Used to add Edgee's materialized skill directories alongside the
/// user's own inside a merge directory.
#[cfg(unix)]
pub fn link_children(from: &Path, into: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(from) else {
        return Ok(());
    };
    fs::create_dir_all(into)?;
    for entry in entries.flatten() {
        let target = into.join(entry.file_name());
        // Ours wins on a name clash; the namespacing makes one unlikely.
        let _ = fs::remove_file(&target);
        let _ = std::os::unix::fs::symlink(entry.path(), &target);
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn link_children(_from: &Path, _into: &Path) -> Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn plain_entries_are_symlinked_not_copied() {
        let source = dir();
        let dest = dir();
        fs::write(source.path().join("auth.json"), "SECRET").unwrap();

        build(source.path(), &dest.path().join("m"), &[]).unwrap();

        let link = dest.path().join("m/auth.json");
        // A symlink, so the credential is never duplicated on disk.
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&link).unwrap(), "SECRET");
    }

    /// The point of a merge directory: we can add siblings without shadowing
    /// what the user already has there.
    #[test]
    fn merge_dirs_are_real_with_children_linked() {
        let source = dir();
        let dest = dir();
        fs::create_dir_all(source.path().join("skills/theirs")).unwrap();
        fs::write(source.path().join("skills/theirs/SKILL.md"), "theirs").unwrap();

        let mirror = dest.path().join("m");
        build(source.path(), &mirror, &["skills"]).unwrap();

        let skills = mirror.join("skills");
        assert!(fs::symlink_metadata(&skills).unwrap().is_dir());
        assert!(!fs::symlink_metadata(&skills)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(skills.join("theirs/SKILL.md")).unwrap(),
            "theirs"
        );

        // and ours can be added beside theirs
        let ours = dir();
        fs::create_dir_all(ours.path().join("edgee__x")).unwrap();
        fs::write(ours.path().join("edgee__x/SKILL.md"), "ours").unwrap();
        link_children(ours.path(), &skills).unwrap();

        assert_eq!(
            fs::read_to_string(skills.join("edgee__x/SKILL.md")).unwrap(),
            "ours"
        );
        assert!(skills.join("theirs/SKILL.md").exists());
    }

    #[test]
    fn a_missing_source_still_yields_the_merge_dirs() {
        let dest = dir();
        let mirror = dest.path().join("m");

        build(Path::new("/nonexistent/edgee"), &mirror, &["skills"]).unwrap();

        assert!(mirror.join("skills").is_dir());
    }

    #[test]
    fn rebuilding_clears_stale_entries() {
        let source = dir();
        let dest = dir();
        let mirror = dest.path().join("m");
        fs::write(source.path().join("a"), "a").unwrap();
        build(source.path(), &mirror, &[]).unwrap();

        fs::remove_file(source.path().join("a")).unwrap();
        fs::write(source.path().join("b"), "b").unwrap();
        build(source.path(), &mirror, &[]).unwrap();

        assert!(!mirror.join("a").exists());
        assert!(mirror.join("b").exists());
    }

    /// Writing through the mirror must never reach the user's original.
    #[test]
    fn adding_to_a_merge_dir_does_not_touch_the_source() {
        let source = dir();
        let dest = dir();
        fs::create_dir_all(source.path().join("skills/theirs")).unwrap();
        let mirror = dest.path().join("m");
        build(source.path(), &mirror, &["skills"]).unwrap();

        fs::create_dir_all(mirror.join("skills/ours")).unwrap();

        assert!(!source.path().join("skills/ours").exists());
    }
}
