//! Session-only Git excludes for synthetic Linux protection placeholders.
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use super::RuntimeSpec;

pub(super) fn write(
    spec: &RuntimeSpec,
    created: &[(PathBuf, bool)],
    private_home: &Path,
) -> Result<Option<tempfile::NamedTempFile>> {
    let mut patterns = std::collections::BTreeSet::new();
    for (path, directory) in created {
        let Ok(relative) = path.strip_prefix(&spec.workspace) else {
            continue;
        };
        if relative.as_os_str().is_empty()
            || relative.components().any(|part| part.as_os_str() == ".git")
        {
            continue;
        }
        anyhow::ensure!(
            relative
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
            "Synthetic sandbox paths must stay inside the workspace"
        );
        let value = relative
            .to_str()
            .context("Sandbox Git protection requires UTF-8 project paths")?;
        anyhow::ensure!(
            !value.contains(['\n', '\r']),
            "Sandbox Git protection does not support line breaks in configuration paths"
        );
        let mut pattern = String::from("/");
        for character in value.chars() {
            if matches!(
                character,
                '\\' | '*' | '?' | '[' | ']' | '!' | '#' | ' ' | '\t'
            ) {
                pattern.push('\\');
            }
            pattern.push(character);
        }
        if *directory {
            pattern.push('/');
        }
        patterns.insert(pattern);
    }
    if patterns.is_empty() {
        return Ok(None);
    }
    let existing = configured_excludes(spec, private_home)?;
    let mut file = tempfile::Builder::new()
        .prefix("edgee-git-excludes-")
        .tempfile_in(&spec.session_dir)?;
    if !existing.is_empty() {
        file.write_all(&existing)?;
        file.write_all(b"\n")?;
    }
    for pattern in patterns {
        writeln!(file, "{pattern}")?;
    }
    file.flush()?;
    Ok(Some(file))
}

fn configured_excludes(spec: &RuntimeSpec, private_home: &Path) -> Result<Vec<u8>> {
    let output = Command::new(super::super::native_git()?)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", private_home)
        .env("XDG_CONFIG_HOME", private_home.join(".config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg("-C")
        .arg(&spec.workspace)
        .args(["-c", "core.hooksPath=/dev/null", "-c"])
        .arg(format!("safe.directory={}", spec.workspace.display()))
        .args(["config", "--null", "--path", "--get", "core.excludesfile"])
        .output()
        .context("Cannot inspect project Git excludes; repair Git before sandboxing")?;
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    anyhow::ensure!(output.status.success(), "Cannot read project Git configuration; repair it before sandboxing so existing excludes are preserved");
    let bytes = output
        .stdout
        .strip_suffix(&[0])
        .context("Git returned an invalid excludes pathname")?;
    let value = String::from_utf8(bytes.to_vec()).context("Git excludes pathname must be UTF-8")?;
    let path = spec.workspace.join(value).canonicalize()
        .context("Configured Git core.excludesfile is unavailable; move it into the workspace or an authorized read path before sandboxing")?;
    let allowed = [&spec.workspace, &spec.session_dir]
        .into_iter()
        .chain(spec.read_paths.iter())
        .chain(spec.write_paths.iter())
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| path == root || (root.is_dir() && path.starts_with(&root)));
    anyhow::ensure!(allowed && path.is_file(), "Configured Git core.excludesfile is not readable inside the sandbox; move it into the workspace or ask your administrator to authorize its directory");
    // Do not follow a replacement symlink at the final path component.
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .context("Cannot read configured Git excludes safely; repair the file before sandboxing")?;
    let mut contents = Vec::new();
    file.take(1_048_577).read_to_end(&mut contents)?;
    anyhow::ensure!(
        contents.len() <= 1_048_576,
        "Configured Git excludes exceeds 1 MiB; reduce it before sandboxing"
    );
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn spec(workspace: &Path, session: &Path) -> RuntimeSpec {
        RuntimeSpec {
            workspace: workspace.to_path_buf(),
            cwd: workspace.to_path_buf(),
            session_dir: session.to_path_buf(),
            read_paths: vec![],
            write_paths: vec![],
            deny_write_paths: vec![],
            allowed_domains: vec![],
            tool_path: "/usr/bin:/bin".into(),
        }
    }

    fn git(workspace: &Path, excludes: &Path, args: &[&str]) -> String {
        let output = Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "safe.directory")
            .env("GIT_CONFIG_VALUE_0", workspace)
            .env("GIT_CONFIG_KEY_1", "core.excludesfile")
            .env("GIT_CONFIG_VALUE_1", excludes)
            .arg("-C")
            .arg(workspace)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn only_synthetic_configuration_is_excluded_from_status_and_add() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("project with spaces");
        let session = root.path().join("session");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&session).unwrap();
        git(&workspace, Path::new("/dev/null"), &["init", "--quiet"]);
        let config_before = fs::read(workspace.join(".git/config")).unwrap();
        let excludes_before = fs::read(workspace.join(".git/info/exclude")).unwrap();
        fs::write(workspace.join("actual.txt"), "real change").unwrap();
        fs::write(workspace.join(".gitmodules"), "# preexisting user file\n").unwrap();
        fs::create_dir(workspace.join("nested[ab] *?!# space")).unwrap();
        fs::create_dir(workspace.join("nesteda")).unwrap();
        fs::write(workspace.join("nesteda/.mcp.json"), "{}\n").unwrap();
        let created = vec![
            (workspace.join(".mcp.json"), false),
            (workspace.join(".bashrc"), false),
            (workspace.join("nested[ab] *?!# space/.mcp.json"), false),
            (workspace.join(".claude"), true),
            (workspace.join(".git/config.worktree"), false),
        ];
        for (path, directory) in &created {
            if *directory {
                fs::create_dir(path).unwrap();
            } else {
                fs::write(path, "").unwrap();
            }
        }
        let file = write(&spec(&workspace, &session), &created, &session)
            .unwrap()
            .unwrap();
        let status = git(
            &workspace,
            file.path(),
            &["status", "--porcelain", "--untracked-files=all"],
        );
        assert!(status.contains("actual.txt"));
        assert!(status.contains(".gitmodules"));
        assert!(status.contains("nesteda/.mcp.json"));
        assert!(!status.contains("nested[ab]"));
        assert!(!status.contains("?? .mcp.json"));
        assert!(!status.contains(".bashrc"));
        git(&workspace, file.path(), &["add", "."]);
        let staged = git(
            &workspace,
            file.path(),
            &["diff", "--cached", "--name-only"],
        );
        assert_eq!(
            staged.lines().collect::<Vec<_>>(),
            [".gitmodules", "actual.txt", "nesteda/.mcp.json"]
        );
        assert_eq!(
            fs::read(workspace.join(".git/config")).unwrap(),
            config_before
        );
        assert_eq!(
            fs::read(workspace.join(".git/info/exclude")).unwrap(),
            excludes_before
        );
        let contents = fs::read_to_string(file.path()).unwrap();
        assert!(!contents.contains("/.git/"));
    }

    #[test]
    fn unsupported_pattern_paths_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        for name in ["line\nbreak/.mcp.json", "line\rbreak/.mcp.json"] {
            assert!(write(
                &spec(root.path(), root.path()),
                &[(root.path().join(name), false)],
                root.path()
            )
            .is_err());
        }
    }

    #[test]
    fn preserves_authorized_custom_excludes_and_gitignore() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("repo");
        let session = root.path().join("session");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&session).unwrap();
        git(&workspace, Path::new("/dev/null"), &["init", "--quiet"]);
        fs::write(workspace.join("custom.ignore"), "/custom.secret").unwrap();
        fs::write(workspace.join(".gitignore"), "/normal.secret\n").unwrap();
        fs::write(workspace.join("custom.secret"), "fake secret").unwrap();
        fs::write(workspace.join("normal.secret"), "fake secret").unwrap();
        fs::write(workspace.join("actual.txt"), "real change").unwrap();
        git(
            &workspace,
            Path::new("/dev/null"),
            &["config", "core.excludesfile", "custom.ignore"],
        );
        let config = fs::read(workspace.join(".git/config")).unwrap();
        fs::write(workspace.join(".mcp.json"), "{}").unwrap();
        let created = [(workspace.join(".mcp.json"), false)];
        let file = write(&spec(&workspace, &session), &created, &session)
            .unwrap()
            .unwrap();
        git(&workspace, file.path(), &["add", "."]);
        let staged = git(
            &workspace,
            file.path(),
            &["diff", "--cached", "--name-only"],
        );
        assert_eq!(
            staged.lines().collect::<Vec<_>>(),
            [".gitignore", "actual.txt", "custom.ignore"]
        );
        assert_eq!(fs::read(workspace.join(".git/config")).unwrap(), config);
        assert_eq!(
            fs::read_to_string(workspace.join("custom.ignore")).unwrap(),
            "/custom.secret"
        );
    }

    #[test]
    fn excludes_outside_authorized_roots_are_refused_without_silent_override() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("repo");
        let session = root.path().join("session");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&session).unwrap();
        git(&workspace, Path::new("/dev/null"), &["init", "--quiet"]);
        let external = root.path().join("external.ignore");
        fs::write(&external, "*.secret\n").unwrap();
        git(
            &workspace,
            Path::new("/dev/null"),
            &["config", "core.excludesfile", external.to_str().unwrap()],
        );
        let created = [(workspace.join(".mcp.json"), false)];
        let mut policy = spec(&workspace, &session);
        let error = write(&policy, &created, &session).unwrap_err();
        assert!(error
            .to_string()
            .contains("not readable inside the sandbox"));
        policy.read_paths.push(external.clone());
        let preserved = write(&policy, &created, &session).unwrap().unwrap();
        assert!(fs::read_to_string(preserved.path())
            .unwrap()
            .starts_with("*.secret\n"));
    }
}
