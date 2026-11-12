//! Trusted machine policy and local workspace selection. Repository files are
//! deliberately never consulted for security policy.
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

const TOOL_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

#[derive(Debug, Default, clap::Args)]
#[group(id = "sandbox-options")]
pub struct Options {
    /// Isolate the agent and its descendants locally.
    #[arg(long)]
    pub sandbox: bool,
    /// Existing directory to expose read/write to the sandbox.
    #[arg(long, conflicts_with = "repo")]
    pub workspace: Option<PathBuf>,
    /// Clone an HTTPS repository into a new persistent local workspace.
    #[arg(long, conflicts_with = "workspace")]
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub mandatory: bool,
    pub allowed_domains: Vec<String>,
    pub read_paths: Vec<PathBuf>,
    pub node_binary: Option<PathBuf>,
    pub runtime_root: Option<PathBuf>,
    #[serde(skip)]
    pub tool_path: String,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            mandatory: false,
            allowed_domains: vec!["api.edgee.ai".into(), "chatgpt.com".into()],
            read_paths: Vec::new(),
            node_binary: None,
            runtime_root: None,
            tool_path: TOOL_PATH.into(),
        }
    }
}

#[derive(Debug)]
pub struct Prepared {
    pub workspace: PathBuf,
    pub cwd: PathBuf,
    pub policy: Policy,
    /// External worktree administration directories, exposed read-only.
    pub git_metadata: Vec<PathBuf>,
    /// Clone only through the same OS/runtime policy after runtime preflight.
    pub clone_url: Option<String>,
}

impl Policy {
    pub fn load() -> Result<Self> {
        #[cfg(target_os = "macos")]
        let path = Path::new("/private/etc/edgee/sandbox.toml");
        #[cfg(not(target_os = "macos"))]
        let path = Path::new("/etc/edgee/sandbox.toml");
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).context("Cannot inspect administrator sandbox policy"),
            Ok(_) => {}
        }
        validate_admin_path(path)?;
        let mut policy: Self = toml::from_str(&std::fs::read_to_string(path)?)
            .context("Invalid administrator sandbox policy; ask your administrator to fix it")?;
        if policy.mandatory && (policy.node_binary.is_none() || policy.runtime_root.is_none()) {
            bail!(
                "Mandatory sandbox policy must configure root-owned node_binary and runtime_root"
            );
        }
        for domain in &mut policy.allowed_domains {
            *domain = domain.to_ascii_lowercase();
            if !valid_domain(domain) {
                bail!("Sandbox allowed_domains must contain exact DNS hostnames, without wildcards, ports or URLs");
            }
        }
        for path in &mut policy.read_paths {
            if !path.is_absolute() {
                bail!("Administrator sandbox read_paths must be absolute directories");
            }
            *path = validate_scope(path)?;
        }
        for path in [&mut policy.node_binary, &mut policy.runtime_root]
            .into_iter()
            .flatten()
        {
            if !path.is_absolute() {
                bail!("Administrator runtime_root and node_binary must be absolute paths");
            }
            // Check original components before canonicalization can hide symlinks.
            if policy.mandatory {
                validate_admin_path(path)?;
            }
            *path = path
                .canonicalize()
                .context("Administrator sandbox runtime path does not exist")?;
        }
        if policy.node_binary.as_ref().is_some_and(|p| !p.is_file())
            || policy.runtime_root.as_ref().is_some_and(|p| !p.is_dir())
        {
            bail!("Sandbox node_binary must be a file and runtime_root must be a directory");
        }
        if policy.mandatory {
            validate_admin_tree(policy.runtime_root.as_ref().expect("required above"))?;
            policy.tool_path = trusted_tool_path()?;
        }
        Ok(policy)
    }
}

#[cfg(unix)]
fn trusted_tool_path() -> Result<String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let directories: Vec<PathBuf> = TOOL_PATH
        .split(':')
        .map(PathBuf::from)
        .filter(|path| path.is_dir() && validate_trusted_alias(path).is_ok())
        .collect();
    let required: &[&str] = if cfg!(target_os = "linux") {
        &["rg", "bwrap", "socat"]
    } else {
        &["rg"]
    };
    for helper in required {
        let path = directories.iter().map(|dir| dir.join(helper)).find(|path| {
            path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        }).with_context(|| format!("Mandatory sandbox requires a root-owned {helper} in a trusted system PATH directory; ask your administrator to install it"))?;
        if std::fs::symlink_metadata(&path)?.uid() != 0 {
            bail!(
                "Mandatory sandbox helper must be root-owned: {}",
                path.display()
            );
        }
        validate_trusted_alias(&path)?;
    }
    Ok(std::env::join_paths(directories)?
        .to_string_lossy()
        .into_owned())
}

#[cfg(not(unix))]
fn trusted_tool_path() -> Result<String> {
    bail!("Mandatory local sandbox requires macOS or Linux")
}

/// System tool aliases such as Linux /bin -> /usr/bin are legitimate only if
/// both the link's entire original chain and its canonical target are trusted.
#[cfg(unix)]
fn validate_trusted_alias(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)?;
        if metadata.uid() != 0
            || (!metadata.file_type().is_symlink() && metadata.mode() & 0o022 != 0)
        {
            bail!("System tool paths must be root-owned and not writable by other users");
        }
        reject_extended_acl(ancestor)?;
    }
    validate_admin_path(&path.canonicalize()?)
}

fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
        && domain.parse::<std::net::IpAddr>().is_err()
        && domain != "localhost"
        && !domain.ends_with(".localhost")
        && domain.contains('.')
        && !domain.ends_with(".local")
        && !domain.ends_with(".internal")
        && !domain.ends_with(".lan")
}

fn validate_admin_path(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)
            .context("Cannot verify ownership of administrator sandbox policy")?;
        if metadata.file_type().is_symlink() {
            bail!("Administrator sandbox policy and its parents must not be symlinks");
        }
        reject_extended_acl(ancestor)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                bail!("Administrator sandbox policy and its parents must be root-owned and not group/world writable");
            }
        }
        #[cfg(not(unix))]
        bail!("Administrator sandbox policy is supported only on macOS and Linux");
    }
    Ok(())
}

/// Runtime dependencies are executable policy-enforcement code too. Checking
/// only the package root would allow a writable dependency to disable isolation.
#[cfg(unix)]
fn validate_admin_tree(root: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let mut pending = vec![root.to_path_buf()];
    let mut inspected = 0_usize;
    while let Some(path) = pending.pop() {
        inspected += 1;
        if inspected > 100_000 {
            bail!("Sandbox runtime contains too many files to validate safely");
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.uid() != 0 {
            bail!(
                "Every sandbox runtime file must be root-owned: {}",
                path.display()
            );
        }
        if metadata.file_type().is_symlink() {
            let target = path
                .canonicalize()
                .context("Sandbox runtime contains a broken symlink")?;
            if !target.starts_with(root) {
                bail!(
                    "Sandbox runtime symlink escapes its root: {}",
                    path.display()
                );
            }
            validate_admin_path(&target)?;
        } else {
            reject_extended_acl(&path)?;
            if metadata.mode() & 0o022 != 0 {
                bail!(
                    "Sandbox runtime must not be group/world writable: {}",
                    path.display()
                );
            }
            if metadata.is_dir() {
                for entry in std::fs::read_dir(&path)? {
                    pending.push(entry?.path());
                }
            } else if !metadata.is_file() {
                bail!(
                    "Sandbox runtime contains an unsupported special file: {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_admin_tree(_root: &Path) -> Result<()> {
    bail!("Mandatory local sandbox is supported only on macOS and Linux")
}

#[cfg(target_os = "macos")]
fn reject_extended_acl(path: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // Darwin's ACL API is not currently exported by the libc crate. Constants
    // and signatures are from the platform SDK's sys/acl.h.
    unsafe extern "C" {
        fn acl_get_file(path: *const libc::c_char, kind: libc::c_int) -> *mut libc::c_void;
        fn acl_get_entry(
            acl: *mut libc::c_void,
            id: libc::c_int,
            entry: *mut *mut libc::c_void,
        ) -> libc::c_int;
        fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
    }
    let name = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: name is NUL-terminated; the owned ACL is freed exactly once after
    // the only borrowed entry pointer has stopped being used.
    let acl = unsafe { acl_get_file(name.as_ptr(), 0x100) };
    if acl.is_null() {
        // Darwin reports ENOENT for a missing extended ACL even when the file
        // exists. Recheck the object so a missing path never counts as trusted.
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT)
            && std::fs::symlink_metadata(path).is_ok()
        {
            return Ok(());
        }
        bail!("Cannot verify administrator path ACL: {}", path.display());
    }
    let mut entry = std::ptr::null_mut();
    let result = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let error = std::io::Error::last_os_error();
    unsafe {
        acl_free(acl);
    }
    if result == 0 {
        bail!(
            "Administrator sandbox paths must not have extended ACLs: {}",
            path.display()
        );
    }
    if error.raw_os_error() != Some(libc::EINVAL) {
        bail!("Cannot inspect administrator path ACL: {}", path.display());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn reject_extended_acl(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn prepare(options: &Options) -> Result<Option<Prepared>> {
    let policy = Policy::load()?;
    if !options.sandbox && !policy.mandatory {
        if options.workspace.is_some() || options.repo.is_some() {
            bail!("--workspace and --repo require --sandbox before the agent name");
        }
        return Ok(None);
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        bail!("Local sandbox supports macOS and Linux only; use a supported host.");
    }
    let original = std::env::current_dir()?.canonicalize()?;
    let workspace = if let Some(path) = &options.workspace {
        validate_scope(path)?
    } else if let Some(url) = &options.repo {
        reserve_clone_workspace(url)?
    } else if let Some(root) = git_path(&original, "--show-toplevel") {
        validate_scope(&root)?
    } else if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        let answer: String = dialoguer::Input::new()
            .with_prompt("Sandbox workspace directory")
            .interact_text()?;
        validate_scope(Path::new(&answer))?
    } else {
        bail!("No Git workspace detected. Pass --sandbox --workspace <directory> before the agent name");
    };
    let mut protected = vec![PathBuf::from("/etc"), PathBuf::from("/private/etc")];
    if let Some(home) = account_home() {
        protected.extend([
            home.join(".config/edgee"),
            home.join(".local/share/edgee/sandbox-runtime"),
        ]);
    }
    if let Some(runtime) = &policy.runtime_root {
        protected.push(runtime.clone());
    }
    for path in protected {
        if let Ok(path) = path.canonicalize() {
            if workspace.starts_with(&path) || path.starts_with(&workspace) {
                bail!("Workspace overlaps sandbox policy or runtime files; choose a separate project directory");
            }
        }
    }
    let cwd = if original.starts_with(&workspace) {
        original
    } else {
        workspace.clone()
    };
    let mut git_metadata = Vec::new();
    for flag in ["--absolute-git-dir", "--git-common-dir"] {
        if let Some(path) = git_path(&workspace, flag) {
            let path = validate_scope(&path)?;
            if !path.starts_with(&workspace) && !git_metadata.contains(&path) {
                git_metadata.push(path);
            }
        }
    }
    Ok(Some(Prepared {
        workspace,
        cwd,
        policy,
        git_metadata,
        clone_url: options.repo.clone(),
    }))
}

fn validate_scope(path: &Path) -> Result<PathBuf> {
    #[cfg(unix)]
    if account_home().is_none() {
        bail!("Cannot determine the account home directory; refusing to select an unsafe sandbox scope");
    }
    let path = path.canonicalize().with_context(|| {
        format!(
            "Cannot open sandbox directory {}; choose an existing directory",
            path.display()
        )
    })?;
    if !path.is_dir() {
        bail!("Sandbox access paths must be directories");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok());
    if path.parent().is_none()
        || home.as_ref().is_some_and(|h| h.starts_with(&path))
        || account_home()
            .as_ref()
            .is_some_and(|h| h.starts_with(&path))
    {
        bail!("Sandbox scope is too broad: choose a project directory, not your home or its ancestors");
    }
    Ok(path)
}

/// Consult the account database, not caller-controlled HOME, for the protected
/// account root. The reentrant API avoids races with concurrent auth tasks.
#[cfg(unix)]
fn account_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;

    let mut size = 16_384;
    loop {
        let mut buffer = vec![0_u8; size];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: buffers live until the returned home string is copied;
        // getpwuid_r initializes entry only on successful nonnull result.
        let status = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && size < 1_048_576 {
            size *= 2;
            continue;
        }
        if status != 0 || result.is_null() {
            return None;
        }
        let dir = unsafe { (*result).pw_dir };
        if dir.is_null() {
            return None;
        }
        let value = unsafe { CStr::from_ptr(dir) };
        return PathBuf::from(std::ffi::OsStr::from_bytes(value.to_bytes()))
            .canonicalize()
            .ok();
    }
}

#[cfg(not(unix))]
fn account_home() -> Option<PathBuf> {
    None
}

fn git_command(cwd: &Path) -> Command {
    let mut cmd = Command::new("/usr/bin/git");
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .current_dir(cwd);
    cmd
}

fn git_path(cwd: &Path, flag: &str) -> Option<PathBuf> {
    let out = git_command(cwd).args(["rev-parse", flag]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8(out.stdout).ok()?;
    let path = PathBuf::from(value.trim());
    cwd.join(path).canonicalize().ok()
}

fn reserve_clone_workspace(value: &str) -> Result<PathBuf> {
    // Only reserve a fresh empty directory on the host. The launcher performs
    // the actual network clone through the OS sandbox and its network policy.
    let url = reqwest::Url::parse(value).context("--repo requires an HTTPS repository URL")?;
    if url.scheme() != "https"
        || !url.host_str().is_some_and(valid_domain)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        bail!("--repo requires an HTTPS URL without credentials, query or fragment; clone private repositories separately and use --workspace");
    }
    let home = std::env::var_os("HOME").context("HOME is unavailable; use --workspace")?;
    let base = PathBuf::from(home).join(".edgee/workspaces");
    std::fs::create_dir_all(&base).context("Cannot create clone workspace directory")?;
    let destination = base.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&destination).context("Cannot reserve a fresh clone workspace")?;
    validate_scope(&destination)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn sandbox_flags_stay_before_target_and_native_flags_are_untouched() {
        use crate::commands::launch::Command as Agent;

        for target in ["claude", "codex"] {
            let options = crate::Options::try_parse_from([
                "edgee",
                "launch",
                "--sandbox",
                "--workspace",
                "/project with spaces",
                target,
                "--sandbox",
                "native-value",
                "--workspace",
                "native-dir",
                "--repo",
                "native-repo",
                "-p",
                "prompt with spaces",
            ])
            .unwrap();
            let crate::commands::Command::Launch(launch) = options.command else {
                panic!("wrong command")
            };
            assert!(launch.sandbox.sandbox);
            assert_eq!(
                launch.sandbox.workspace,
                Some(PathBuf::from("/project with spaces"))
            );
            let args = match launch.command {
                Agent::Claude(o) => o.args,
                Agent::Codex(o) => o.args,
                _ => panic!("wrong target"),
            };
            assert_eq!(
                args,
                [
                    "--sandbox",
                    "native-value",
                    "--workspace",
                    "native-dir",
                    "--repo",
                    "native-repo",
                    "-p",
                    "prompt with spaces"
                ]
            );
        }
    }

    #[test]
    fn workspace_and_repo_are_mutually_exclusive() {
        assert!(crate::Options::try_parse_from([
            "edgee",
            "launch",
            "--sandbox",
            "--workspace",
            "/project",
            "--repo",
            "https://example.com/repo.git",
            "claude"
        ])
        .is_err());
    }

    #[test]
    fn unchanged_alias_separator_preserves_native_sandbox_flag() {
        let options = crate::Options::try_parse_from([
            "edgee",
            "launch",
            "codex",
            "--",
            "--sandbox",
            "workspace-write",
        ])
        .unwrap();
        let crate::commands::Command::Launch(launch) = options.command else {
            panic!("wrong command")
        };
        assert!(!launch.sandbox.sandbox);
        let crate::commands::launch::Command::Codex(agent) = launch.command else {
            panic!("wrong target")
        };
        assert_eq!(agent.args, ["--sandbox", "workspace-write"]);
    }

    #[test]
    fn policy_rejects_unknown_fields_and_unsafe_domains() {
        assert!(toml::from_str::<Policy>("disable = true").is_err());
        assert_eq!(toml::from_str::<Policy>("").unwrap().tool_path, TOOL_PATH);
        for bad in [
            "",
            "*.example.com",
            "https://example.com",
            "example.com:443",
            "localhost",
            "a.localhost",
            "127.0.0.1",
            "a..com",
            "-bad.com",
        ] {
            assert!(!valid_domain(bad), "{bad}");
        }
        assert!(valid_domain("api.edgee.ai"));
        assert!(valid_domain("registry.npmjs.org"));
    }

    #[test]
    fn broad_and_missing_scopes_are_rejected() {
        assert!(validate_scope(Path::new("/")).is_err());
        if let Some(home) = std::env::var_os("HOME") {
            assert!(validate_scope(Path::new(&home)).is_err());
        }
        if let Some(home) = account_home() {
            assert!(validate_scope(&home).is_err());
        }
        let tmp = tempfile::tempdir().unwrap();
        assert!(validate_scope(&tmp.path().join("missing")).is_err());
        assert_eq!(
            validate_scope(tmp.path()).unwrap(),
            tmp.path().canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_broad_scope_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/", tmp.path().join("root")).unwrap();
        assert!(validate_scope(&tmp.path().join("root")).is_err());
        assert!(validate_admin_path(&tmp.path().join("root")).is_err());
        assert!(validate_admin_tree(tmp.path()).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn administrator_paths_reject_extended_acl_grants() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        reject_extended_acl(tmp.path()).unwrap();
        assert!(Command::new("/bin/chmod")
            .args(["+a", "everyone allow write"])
            .arg(tmp.path())
            .status()
            .unwrap()
            .success());
        assert!(reject_extended_acl(tmp.path()).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn git_root_and_worktree_metadata_are_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("project with spaces");
        std::fs::create_dir(&repo).unwrap();
        assert!(git_command(&repo)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success());
        let sub = repo.join("nested");
        std::fs::create_dir(&sub).unwrap();
        assert_eq!(
            git_path(&sub, "--show-toplevel").unwrap(),
            repo.canonicalize().unwrap()
        );
        assert_eq!(
            git_path(&sub, "--absolute-git-dir").unwrap(),
            repo.join(".git").canonicalize().unwrap()
        );
        assert!(git_command(&repo)
            .args([
                "-c",
                "user.name=Sandbox Test",
                "-c",
                "user.email=sandbox@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "fixture"
            ])
            .status()
            .unwrap()
            .success());
        let worktree = tmp.path().join("linked worktree");
        assert!(git_command(&repo)
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&worktree)
            .status()
            .unwrap()
            .success());
        assert_eq!(
            git_path(&worktree, "--show-toplevel").unwrap(),
            worktree.canonicalize().unwrap()
        );
        assert_eq!(
            git_path(&worktree, "--git-common-dir").unwrap(),
            repo.join(".git").canonicalize().unwrap()
        );
        assert!(git_path(&worktree, "--absolute-git-dir")
            .unwrap()
            .starts_with(repo.join(".git").canonicalize().unwrap()));
    }
}
