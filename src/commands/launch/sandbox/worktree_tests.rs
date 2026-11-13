//! Regression tests for selecting external writable Git administration paths.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;

use super::*;

struct Fixture {
    _temp: tempfile::TempDir,
    main: PathBuf,
    common: PathBuf,
    gitdir: PathBuf,
    prepared: policy::Prepared,
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = ProcessCommand::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Sandbox Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Sandbox Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let main = base.join("main");
        let workspace = base.join("linked worktree");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-b", "main"]);
        fs::write(main.join("file"), "initial\n").unwrap();
        git(&main, &["add", "file"]);
        git(&main, &["commit", "-m", "initial"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "sandbox",
                workspace.to_str().unwrap(),
            ],
        );
        let common = main.join(".git").canonicalize().unwrap();
        let gitdir = PathBuf::from(git(&workspace, &["rev-parse", "--absolute-git-dir"]))
            .canonicalize()
            .unwrap();
        let prepared = policy::Prepared {
            cwd: workspace.clone(),
            workspace,
            policy: policy::Policy::default(),
            git_metadata: vec![gitdir.clone(), common.clone()],
            clone_url: None,
        };
        Self {
            _temp: temp,
            main,
            common,
            gitdir,
            prepared,
        }
    }

    fn other_worktree(&self) -> (PathBuf, PathBuf) {
        let other = self.main.parent().unwrap().join("other worktree");
        git(
            &self.main,
            &["worktree", "add", "-b", "other", other.to_str().unwrap()],
        );
        let gitdir = PathBuf::from(git(&other, &["rev-parse", "--absolute-git-dir"]))
            .canonicalize()
            .unwrap();
        (other, gitdir)
    }
}

#[test]
fn linked_worktree_grants_only_current_administration_and_shared_git_data() {
    let fixture = Fixture::new();
    let (writes, denies) = worktree_access(&fixture.prepared).unwrap();
    assert_eq!(writes.len(), 4);
    assert!(writes.contains(&fixture.gitdir));
    for name in ["objects", "refs", "logs"] {
        assert!(writes.contains(&fixture.common.join(name)));
    }
    assert!(!writes.contains(&fixture.common));
    assert!(!writes.contains(&fixture.common.join("worktrees")));
    assert!(denies.contains(&fixture.common.join("objects/info")));
    for name in ["config", "config.worktree", "hooks", "commondir", "gitdir"] {
        assert!(denies.contains(&fixture.gitdir.join(name)));
    }
}

#[test]
fn shared_git_write_roots_cannot_be_symlinks_to_other_administration() {
    for (name, target) in [("objects", "."), ("refs", "hooks"), ("logs", "worktrees")] {
        let fixture = Fixture::new();
        let candidate = fixture.common.join(name);
        fs::rename(&candidate, fixture.common.join(format!("saved-{name}"))).unwrap();
        symlink(target, &candidate).unwrap();
        assert!(
            worktree_access(&fixture.prepared).is_err(),
            "{name} -> {target}"
        );
    }
}

#[test]
fn workspace_git_pointer_cannot_alias_another_worktree() {
    let mut fixture = Fixture::new();
    let (other, other_gitdir) = fixture.other_worktree();
    let pointer = fixture.prepared.workspace.join(".git");
    fs::remove_file(&pointer).unwrap();
    symlink(other.join(".git"), &pointer).unwrap();
    // This is what Git detects through the symlink; canonical backlink equality
    // alone would mistakenly grant the other worktree's administration.
    fixture.prepared.git_metadata = vec![other_gitdir, fixture.common.clone()];
    assert!(worktree_access(&fixture.prepared).is_err());
}

#[test]
fn common_worktrees_directory_cannot_redirect_writes() {
    let mut fixture = Fixture::new();
    let original = fixture.common.join("worktrees");
    let moved = fixture.common.join("moved-worktrees");
    fs::rename(&original, &moved).unwrap();
    symlink(&moved, &original).unwrap();
    fixture.prepared.git_metadata[0] = fixture.gitdir.canonicalize().unwrap();
    assert!(worktree_access(&fixture.prepared).is_err());
}

#[test]
fn external_git_administration_must_point_back_to_selected_workspace() {
    let fixture = Fixture::new();
    let (other, _) = fixture.other_worktree();
    fs::write(
        fixture.gitdir.join("gitdir"),
        format!("{}\n", other.join(".git").display()),
    )
    .unwrap();
    assert!(worktree_access(&fixture.prepared).is_err());
}

#[test]
fn existing_nested_security_configurations_are_protected_without_following_links() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let outside = directory.path().join("outside");
    for path in [
        workspace.join("nested/.git"),
        workspace.join("nested/.vscode"),
        outside.join(".git"),
    ] {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(workspace.join("nested/.mcp.json"), "{}").unwrap();
    symlink(&outside, workspace.join("linked-outside")).unwrap();
    let protected = protected_paths(&workspace).unwrap();
    for path in [
        ".mcp.json",
        ".claude",
        ".git/modules",
        "nested/.git/config",
        "nested/.git/hooks",
        "nested/.git/modules",
        "nested/.vscode",
        "nested/.mcp.json",
    ] {
        assert!(protected.contains(&workspace.join(path)), "{path}");
    }
    assert!(!protected.iter().any(
        |path| path.starts_with(&outside) || path.starts_with(workspace.join("linked-outside"))
    ));
    assert!(protected_paths_bounded(&workspace, 1, 4096).is_err());
    assert!(protected_paths_bounded(&workspace, 100, 1).is_err());
}

#[test]
fn protected_security_configs_cannot_alias_writable_workspace_files() {
    for hardlink in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path();
        fs::create_dir(workspace.join(".vscode")).unwrap();
        let target = workspace.join("ordinary-file");
        fs::write(&target, "fake security config").unwrap();
        let config = workspace.join(".vscode/settings.json");
        if hardlink {
            fs::hard_link(&target, &config).unwrap();
        } else {
            symlink(&target, &config).unwrap();
        }
        assert!(protected_paths(workspace).is_err(), "hardlink={hardlink}");
    }
}

#[test]
fn external_worktree_security_configs_reject_writable_aliases() {
    for hardlink in [false, true] {
        let fixture = Fixture::new();
        let ordinary = fixture.gitdir.join("ordinary-file");
        fs::write(&ordinary, "fake configuration").unwrap();
        let protected = fixture.gitdir.join("config.worktree");
        if hardlink {
            fs::hard_link(&ordinary, &protected).unwrap();
        } else {
            symlink(&ordinary, &protected).unwrap();
        }
        assert!(worktree_access(&fixture.prepared).is_err(), "hardlink={hardlink}");
    }
}
