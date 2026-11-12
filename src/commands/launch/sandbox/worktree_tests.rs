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
