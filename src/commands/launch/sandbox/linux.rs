//! Linux namespaces, native proxy bridge and immutable filesystem grafts.
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};

use super::{Payload, RuntimeSpec};

pub(super) const BRIDGE_PORT: u16 = 3128;

pub(super) fn bwrap(tool_path: &str) -> Result<PathBuf> {
    tool_path.split(':').map(|dir| Path::new(dir).join("bwrap"))
        .find(|path| path.metadata().is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0))
        .context("Linux sandbox requires bubblewrap. Install the distribution's bubblewrap package and retry; no unconfined fallback is available.")
}

pub(super) struct HostBridge {
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<(UnixStream, TcpStream)>>>,
    acceptor: Option<JoinHandle<()>>,
}
impl HostBridge {
    pub(super) fn start(socket: &Path, proxy_port: u16) -> Result<Self> {
        let listener = UnixListener::bind(socket)?;
        fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let worker_stop = stop.clone();
        let worker_sockets = sockets.clone();
        let acceptor = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((client, _)) => {
                        if worker_sockets.lock().unwrap().len() >= 128 {
                            continue;
                        }
                        let Ok(upstream) =
                            TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, proxy_port))
                        else {
                            continue;
                        };
                        // Each accepted connection is kept only until its tunnel
                        // finishes, so long sessions do not leak descriptors.
                        let Ok(client_copy) = client.try_clone() else {
                            continue;
                        };
                        let Ok(upstream_copy) = upstream.try_clone() else {
                            continue;
                        };
                        let key = upstream.local_addr().ok();
                        worker_sockets
                            .lock()
                            .unwrap()
                            .push((client_copy, upstream_copy));
                        let sockets = worker_sockets.clone();
                        thread::spawn(move || {
                            bridge_unix_tcp(client, upstream);
                            sockets
                                .lock()
                                .unwrap()
                                .retain(|(_, stream)| stream.local_addr().ok() != key);
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            stop,
            sockets,
            acceptor: Some(acceptor),
        })
    }
}
impl Drop for HostBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.acceptor.take() {
            let _ = worker.join();
        }
        for (unix, tcp) in self.sockets.lock().unwrap().drain(..) {
            let _ = unix.shutdown(Shutdown::Both);
            let _ = tcp.shutdown(Shutdown::Both);
        }
    }
}
fn bridge_unix_tcp(mut unix: UnixStream, mut tcp: TcpStream) {
    let (Ok(mut unix_writer), Ok(mut tcp_reader)) = (unix.try_clone(), tcp.try_clone()) else {
        return;
    };
    let reverse = thread::spawn(move || {
        let _ = io::copy(&mut tcp_reader, &mut unix_writer);
        let _ = unix_writer.shutdown(Shutdown::Write);
    });
    let _ = io::copy(&mut unix, &mut tcp);
    let _ = tcp.shutdown(Shutdown::Write);
    let _ = reverse.join();
}

struct Stub {
    path: PathBuf,
    device: u64,
    inode: u64,
    directory: bool,
    contents: Vec<u8>,
}
pub(super) struct DenyStubs {
    created: Vec<Stub>,
}
impl DenyStubs {
    pub(super) fn created_paths(&self) -> Vec<(PathBuf, bool)> {
        self.created
            .iter()
            .map(|stub| (stub.path.clone(), stub.directory))
            .collect()
    }

    pub(super) fn prepare(spec: &RuntimeSpec) -> Result<Self> {
        let mut guard = Self {
            created: Vec::new(),
        };
        for path in &spec.deny_write_paths {
            if !write_roots(spec).iter().any(|root| path.starts_with(root)) {
                continue;
            }
            for ancestor in path.ancestors() {
                if !write_roots(spec)
                    .iter()
                    .any(|root| ancestor.starts_with(root))
                {
                    break;
                }
                if let Ok(metadata) = fs::symlink_metadata(ancestor) {
                    anyhow::ensure!(!metadata.file_type().is_symlink(), "Protected sandbox paths must not be symlinks: {}; repair the project configuration before launching", ancestor.display());
                }
            }
            if fs::symlink_metadata(path).is_ok() {
                continue;
            }
            let mut missing = Vec::new();
            let mut ancestor = path.parent().context("Protected path has no parent")?;
            while !ancestor.exists() {
                missing.push(ancestor.to_path_buf());
                ancestor = ancestor
                    .parent()
                    .context("Cannot create protected path outside workspace")?;
            }
            for parent in missing.into_iter().rev() {
                guard.create(&parent, true)?;
            }
            guard.create(path, directory_deny(path))?;
        }
        Ok(guard)
    }
    fn create(&mut self, path: &Path, directory: bool) -> Result<()> {
        // An empty commondir makes an ordinary Git repository invalid. A
        // self-reference preserves its existing meaning while denying changes.
        let contents = if path.file_name().is_some_and(|name| name == "commondir") {
            b".\n".to_vec()
        } else if path.file_name().is_some_and(|name| name == ".mcp.json") {
            b"{\"mcpServers\":{}}\n".to_vec()
        } else {
            Vec::new()
        };
        if directory {
            fs::create_dir(path)?;
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?
                .write_all(&contents)?;
        }
        let metadata = fs::symlink_metadata(path)?;
        self.created.push(Stub {
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
            directory,
            contents,
        });
        Ok(())
    }
}
impl Drop for DenyStubs {
    fn drop(&mut self) {
        for stub in self.created.iter().rev() {
            let Ok(metadata) = fs::symlink_metadata(&stub.path) else {
                continue;
            };
            if metadata.dev() != stub.device
                || metadata.ino() != stub.inode
                || metadata.file_type().is_symlink()
            {
                continue;
            }
            if stub.directory {
                let _ = fs::remove_dir(&stub.path);
            } else if metadata.is_file()
                && metadata.len() == stub.contents.len() as u64
                && fs::read(&stub.path).is_ok_and(|data| data == stub.contents)
            {
                let _ = fs::remove_file(&stub.path);
            }
        }
    }
}
fn directory_deny(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(
            ".edgee"
                | ".claude"
                | ".codex"
                | ".vscode"
                | ".idea"
                | "hooks"
                | "info"
                | "worktrees"
                | "modules"
        )
    )
}
fn write_roots(spec: &RuntimeSpec) -> Vec<PathBuf> {
    let mut roots = vec![spec.workspace.clone(), spec.session_dir.clone()];
    roots.extend(spec.write_paths.clone());
    roots
}

pub(super) fn command(
    payload: &Payload,
    spec: &RuntimeSpec,
    control: &Path,
    probe: bool,
) -> Result<Command> {
    let executable = helper_binary()?;
    let payload_path = control.join(if probe { "probe.json" } else { "agent.json" });
    fs::write(&payload_path, serde_json::to_vec(payload)?)?;
    fs::set_permissions(&payload_path, fs::Permissions::from_mode(0o600))?;
    let mut command = Command::new(bwrap(&spec.tool_path)?);
    command.env_clear().env("PATH", &spec.tool_path).args([
        "--unshare-all",
        "--die-with-parent",
        "--as-pid-1",
        "--cap-drop",
        "ALL",
        "--tmpfs",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
    ]);
    // The host's merged-/usr symlinks must retain their spelling for ELF
    // interpreters and shell shebangs while grants use canonical source paths.
    for alias in ["/bin", "/sbin", "/lib", "/lib64"] {
        if fs::symlink_metadata(alias).is_ok_and(|m| m.file_type().is_symlink()) {
            command
                .arg("--symlink")
                .arg(fs::read_link(alias)?)
                .arg(alias);
        }
    }
    let mut reads: BTreeSet<PathBuf> = spec
        .read_paths
        .iter()
        .filter(|p| p.exists())
        .map(|p| p.canonicalize())
        .collect::<io::Result<_>>()?;
    reads.insert(executable.clone());
    // Shared object loader configuration is public runtime data. System read
    // roots already include the actual libraries used by the Edgee helper.
    let loader_cache = PathBuf::from("/etc/ld.so.cache");
    if loader_cache.is_file() {
        reads.insert(loader_cache);
    }
    for source in reads {
        if source.starts_with("/dev") || source.starts_with("/proc") {
            continue;
        }
        command.arg("--ro-bind").arg(&source).arg(&source);
    }
    let writes: BTreeSet<PathBuf> = write_roots(spec).into_iter().collect();
    for source in &writes {
        anyhow::ensure!(
            source.is_absolute() && source.is_dir() && source.canonicalize()? == *source,
            "Sandbox writable roots must be canonical existing directories"
        );
        command.arg("--bind").arg(source).arg(source);
    }
    // An otherwise-writable parent must not be renamed around an immutable
    // child. Self-bind each intermediate parent to make rename fail with EBUSY.
    let mut pinned = BTreeSet::new();
    for denied in &spec.deny_write_paths {
        let mut ancestor = denied.parent();
        while let Some(path) = ancestor {
            if writes.contains(path) {
                break;
            }
            if writes.iter().any(|root| path.starts_with(root)) && path.is_dir() {
                pinned.insert(path.to_path_buf());
            }
            ancestor = path.parent();
        }
    }
    for path in pinned {
        command.arg("--bind").arg(&path).arg(&path);
    }
    let mut masked = HashSet::new();
    for denied in &spec.deny_write_paths {
        // Read-only grants and the private root already deny these writes.
        // Binding an unrelated deny path would accidentally grant it reads.
        if !writes.iter().any(|root| denied.starts_with(root)) {
            continue;
        }
        if !denied.exists() {
            continue;
        }
        let canonical = denied.canonicalize()?;
        for path in [denied.clone(), canonical] {
            if masked.insert(path.clone()) {
                command.arg("--ro-bind").arg(&path).arg(&path);
            }
        }
    }
    command
        .arg("--ro-bind")
        .arg(&payload_path)
        .arg(&payload_path)
        .arg("--ro-bind")
        .arg(&payload.socket)
        .arg(&payload.socket)
        .args(["--remount-ro", "/dev", "--remount-ro", "/"])
        .arg("--chdir")
        .arg(&spec.cwd)
        .arg("--")
        .arg(&executable)
        .arg("sandbox-internal")
        .arg(&payload_path);
    Ok(command)
}
fn helper_binary() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("EDGEE_TEST_BINARY") {
        return PathBuf::from(path)
            .canonicalize()
            .context("Find test Edgee helper binary");
    }
    std::env::current_exe()?
        .canonicalize()
        .context("Find Edgee sandbox helper")
}

pub(super) fn internal(path: &Path) -> Result<()> {
    verify_namespace()?;
    // Non-dumpable PID1 prevents workload access to bridge sockets or privileged
    // helper memory through /proc, ptrace and process_vm APIs.
    anyhow::ensure!(
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) } == 0,
        "Cannot protect native sandbox helper"
    );
    let payload: Payload = serde_json::from_slice(&fs::read(path)?)?;
    anyhow::ensure!(!payload.argv.is_empty(), "Sandbox workload is empty");
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, BRIDGE_PORT))?;
    let socket = payload.socket.clone();
    thread::spawn(move || {
        let active = Arc::new(AtomicUsize::new(0));
        for client in listener.incoming().flatten() {
            if active.fetch_add(1, Ordering::SeqCst) >= 128 {
                active.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let Ok(unix) = UnixStream::connect(&socket) else {
                active.fetch_sub(1, Ordering::SeqCst);
                continue;
            };
            let active = active.clone();
            thread::spawn(move || {
                bridge_unix_tcp(unix, client);
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    let mut command = Command::new(&payload.argv[0]);
    command
        .args(&payload.argv[1..])
        .env_clear()
        .envs(&payload.env)
        .current_dir(&payload.cwd);
    // SAFETY: the seccomp installer uses only kernel syscalls in the child.
    unsafe {
        command.pre_exec(|| {
            // Preserve only explicit stdio through exec, even if a calling
            // application supplied unrelated non-CLOEXEC file descriptors.
            if libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 4_u32) != 0 {
                return Err(io::Error::last_os_error());
            }
            super::seccomp::install()
        });
    }
    let status = super::supervise(command, true)?;
    std::process::exit(
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)),
    );
}

fn verify_namespace() -> Result<()> {
    anyhow::ensure!(
        unsafe { libc::getpid() } == 1,
        "Internal sandbox helper requires a prepared PID namespace; use edgee launch --sandbox"
    );
    let root = b"/\0";
    let mut filesystem: libc::statfs = unsafe { std::mem::zeroed() };
    let mut mounts: libc::statvfs = unsafe { std::mem::zeroed() };
    anyhow::ensure!(
        unsafe { libc::statfs(root.as_ptr().cast(), &mut filesystem) } == 0
            && filesystem.f_type == libc::TMPFS_MAGIC
            && unsafe { libc::statvfs(root.as_ptr().cast(), &mut mounts) } == 0
            && mounts.f_flag & libc::ST_RDONLY != 0,
        "Internal sandbox helper requires an immutable private root filesystem"
    );
    // /sys is deliberately not mounted. getifaddrs inspects kernel namespace
    // interfaces without relying on a host sysfs view.
    let mut entries = std::ptr::null_mut();
    anyhow::ensure!(
        unsafe { libc::getifaddrs(&mut entries) } == 0,
        "Cannot verify private network namespace"
    );
    let mut cursor = entries;
    let mut private = true;
    while !cursor.is_null() {
        let interface = unsafe { &*cursor };
        if interface.ifa_flags & libc::IFF_LOOPBACK as u32 == 0 {
            private = false;
        }
        cursor = interface.ifa_next;
    }
    unsafe {
        libc::freeifaddrs(entries);
    }
    anyhow::ensure!(
        private,
        "Internal sandbox helper requires a private loopback-only network namespace"
    );
    let status = fs::read_to_string("/proc/self/status")?;
    for field in ["CapEff:", "CapPrm:", "CapInh:", "CapAmb:"] {
        let capabilities = status
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .and_then(|value| u64::from_str_radix(value.trim(), 16).ok());
        anyhow::ensure!(
            capabilities == Some(0),
            "Internal sandbox helper must have no retained capabilities"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(
        workspace: PathBuf,
        session_dir: PathBuf,
        deny_write_paths: Vec<PathBuf>,
    ) -> RuntimeSpec {
        RuntimeSpec {
            cwd: workspace.clone(),
            workspace,
            session_dir,
            read_paths: vec![],
            write_paths: vec![],
            deny_write_paths,
            allowed_domains: vec![],
            tool_path: "/usr/bin:/bin".into(),
        }
    }

    #[test]
    fn protected_git_placeholders_preserve_status_and_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("repo");
        let session = root.path().join("session");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&session).unwrap();
        assert!(Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "--quiet"])
            .arg(&workspace)
            .status()
            .unwrap()
            .success());
        let paths: Vec<_> = ["commondir", "gitdir", "config.worktree"]
            .iter()
            .map(|name| workspace.join(".git").join(name))
            .collect();
        let spec = spec(workspace.clone(), session, paths.clone());
        let guard = DenyStubs::prepare(&spec).unwrap();
        assert!(Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(&workspace)
            .args(["status", "--porcelain"])
            .status()
            .unwrap()
            .success());
        assert_eq!(fs::read(workspace.join(".git/commondir")).unwrap(), b".\n");
        drop(guard);
        for path in paths {
            assert!(!path.exists(), "{}", path.display());
        }
        assert!(workspace.join(".git/config").is_file());
    }

    #[test]
    fn protected_placeholder_never_follows_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("repo");
        let outside = root.path().join("outside");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join(".claude")).unwrap();
        let spec = spec(
            workspace.clone(),
            root.path().join("session"),
            vec![workspace.join(".claude/settings.json")],
        );
        assert!(DenyStubs::prepare(&spec).is_err());
        assert!(!outside.join("settings.json").exists());
    }

    #[test]
    fn git_add_omits_real_guard_placeholders_and_cleanup_restores_absence() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("repo");
        let session = root.path().join("session");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&session).unwrap();
        let git = || {
            let mut command = Command::new("/usr/bin/git");
            command
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("-C")
                .arg(&workspace);
            command
        };
        assert!(git().args(["init", "--quiet"]).status().unwrap().success());
        fs::write(workspace.join("actual.txt"), "real project change").unwrap();
        let absent: Vec<_> = [".mcp.json", ".gitmodules", ".bashrc"]
            .iter()
            .map(|name| workspace.join(name))
            .collect();
        let spec = spec(workspace.clone(), session.clone(), absent.clone());
        let guard = DenyStubs::prepare(&spec).unwrap();
        let excludes = super::super::git_excludes::write(&spec, &guard.created_paths(), &session)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(workspace.join(".mcp.json")).unwrap()
            )
            .unwrap(),
            serde_json::json!({"mcpServers": {}})
        );
        let configured = || {
            let mut command = git();
            command
                .env("GIT_CONFIG_COUNT", "2")
                .env("GIT_CONFIG_KEY_0", "safe.directory")
                .env("GIT_CONFIG_VALUE_0", &workspace)
                .env("GIT_CONFIG_KEY_1", "core.excludesfile")
                .env("GIT_CONFIG_VALUE_1", excludes.path());
            command
        };
        let status = configured()
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(String::from_utf8(status.stdout).unwrap(), "?? actual.txt\n");
        assert!(configured().args(["add", "."]).status().unwrap().success());
        let staged = configured()
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert!(staged.status.success());
        assert_eq!(String::from_utf8(staged.stdout).unwrap(), "actual.txt\n");
        drop(guard);
        for path in absent {
            assert!(!path.exists());
        }
        assert!(workspace.join("actual.txt").is_file());
    }
}
