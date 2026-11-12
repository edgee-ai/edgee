//! Real native OS boundary tests, explicitly enabled on macOS/Linux CI.
//! Missing helpers or unavailable isolation fail these tests, never skip them.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::symlink;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;

use super::runtime::{execute, preflight, RuntimeSpec};
use super::*;

const TOOL_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
const PROBE_NAME: &str = "commands::launch::sandbox::native_tests::native_sandbox_agent_probe";

struct Fixture {
    _directory: tempfile::TempDir,
    base: PathBuf,
    workspace: PathBuf,
    session: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        preflight(TOOL_PATH).expect("Native sandbox dependencies must be available");
        let directory = tempfile::Builder::new().prefix("ens-").tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let workspace = base.join("workspace with spaces");
        let session = base.join("session");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&session).unwrap();
        Self {
            _directory: directory,
            base,
            workspace,
            session,
        }
    }

    fn spec(&self) -> RuntimeSpec {
        let mut read_paths = system_read_paths();
        read_paths.push(std::env::current_exe().unwrap().canonicalize().unwrap());
        let git = native_git().unwrap();
        read_paths.push(git.parent().unwrap().parent().unwrap().to_path_buf());
        RuntimeSpec {
            workspace: self.workspace.clone(),
            cwd: self.workspace.clone(),
            session_dir: self.session.clone(),
            read_paths,
            write_paths: vec![],
            deny_write_paths: protected_paths(&self.workspace).unwrap(),
            allowed_domains: vec![],
            tool_path: TOOL_PATH.to_owned(),
        }
    }

    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> ProcessCommand {
        let mut command = ProcessCommand::new(program);
        // Deliberately do not env_clear: execute must discard inherited values.
        command
            .env("HOME", &self.session)
            .env("TMPDIR", &self.session)
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }
}

struct ControlledServer {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ControlledServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let ended = stop.clone();
        let thread = thread::spawn(move || {
            while !ended.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut buffer = [0_u8; 4096];
                        if stream.read(&mut buffer).is_ok() {
                            let body = b"controlled-relay";
                            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = stream.write_all(body);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("Controlled server failed: {error}"),
                }
            }
        });
        Self {
            address,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for ControlledServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[cfg(target_os = "macos")]
struct FakeKeychain(PathBuf);

#[cfg(target_os = "macos")]
impl FakeKeychain {
    fn new(path: PathBuf) -> Self {
        let name = path.to_str().unwrap();
        for args in [
            vec!["create-keychain", "-p", "fake-test-password", name],
            vec![
                "add-generic-password",
                "-a",
                "edgee-sandbox-test",
                "-s",
                "edgee-fake-secret",
                "-w",
                "fake-value",
                name,
            ],
        ] {
            assert!(ProcessCommand::new("/usr/bin/security")
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let value = ProcessCommand::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-a",
                "edgee-sandbox-test",
                "-s",
                "edgee-fake-secret",
                "-w",
                name,
            ])
            .output()
            .unwrap();
        assert!(value.status.success());
        assert_eq!(
            String::from_utf8(value.stdout).unwrap().trim(),
            "fake-value"
        );
        Self(path)
    }
}

#[cfg(target_os = "macos")]
impl Drop for FakeKeychain {
    fn drop(&mut self) {
        let _ = ProcessCommand::new("/usr/bin/security")
            .arg("delete-keychain")
            .arg(&self.0)
            .status();
    }
}

#[test]
fn native_sandbox_files_network_secrets_and_local_mcp() {
    assert_eq!(
        std::env::var("FAKE_HOST_SECRET").as_deref(),
        Ok("host-only-fake-secret"),
        "Run the native suite with FAKE_HOST_SECRET=host-only-fake-secret"
    );
    let fixture = Fixture::new();
    std::fs::write(fixture.base.join("secret"), "fake-file-secret").unwrap();
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let secret = std::fs::File::open(fixture.base.join("secret")).unwrap();
    // F_DUPFD deliberately leaves CLOEXEC unset. Keep ownership through launch.
    let raw_fd = unsafe { libc::fcntl(secret.as_raw_fd(), libc::F_DUPFD, 200) };
    assert!(raw_fd >= 200);
    let inherited = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    std::fs::create_dir(fixture.workspace.join(".git")).unwrap();
    std::fs::write(fixture.workspace.join(".git/config"), "# protected\n").unwrap();
    for name in ["nested/.git", "nested/.vscode", ".git/modules/subrepo"] {
        std::fs::create_dir_all(fixture.workspace.join(name)).unwrap();
    }
    for name in [
        "nested/.git/config",
        "nested/.vscode/settings.json",
        ".git/modules/subrepo/config",
    ] {
        std::fs::write(fixture.workspace.join(name), "protected fixture").unwrap();
    }
    let server = ControlledServer::new();
    // Place the socket inside readable scope: this tests socket isolation,
    // not an incidental outside-path read denial.
    let socket = fixture.workspace.join("s");
    let _unix = UnixListener::bind(&socket).unwrap();
    #[cfg(target_os = "macos")]
    let keychain = FakeKeychain::new(fixture.workspace.join("fake.keychain-db"));
    let mut spec = fixture.spec();
    // Production policy rejects IP literals. The trusted RuntimeSpec seam
    // permits this exact controlled endpoint to test the authenticated proxy.
    spec.allowed_domains = vec![server.address.to_string()];
    let mut command = fixture.command(std::env::current_exe().unwrap());
    command
        .args(["--exact", PROBE_NAME, "--nocapture", "--test-threads=1"])
        .env("EDGEE_NATIVE_PROBE", "1")
        .env("EDGEE_TEST_BASE", &fixture.base)
        .env("EDGEE_TEST_ENDPOINT", server.address.to_string())
        .env("EDGEE_TEST_SOCKET", &socket)
        .env("EDGEE_TEST_INHERITED_FD", inherited.as_raw_fd().to_string())
        .env("EXPLICIT_TEST_VALUE", "allowed-fake-value");
    #[cfg(target_os = "macos")]
    command.env("EDGEE_TEST_KEYCHAIN", &keychain.0);
    assert!(
        execute(&command, &spec).unwrap().success(),
        "Native OS probe must pass"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.base.join("secret")).unwrap(),
        "fake-file-secret"
    );
    assert!(!fixture.base.join("escaped").exists());
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("allowed")).unwrap(),
        "okay"
    );
}

// Re-executed as the untrusted workload; the marker is passed explicitly by
// the parent fixture and is absent during ordinary test enumeration.
#[test]
fn native_sandbox_agent_probe() {
    if std::env::var("EDGEE_NATIVE_PROBE").as_deref() != Ok("1") {
        return;
    }
    assert!(std::env::var_os("FAKE_HOST_SECRET").is_none());
    let inherited_fd: i32 = std::env::var("EDGEE_TEST_INHERITED_FD").unwrap().parse().unwrap();
    let mut secret_byte = [0_u8];
    assert_eq!(unsafe { libc::read(inherited_fd, secret_byte.as_mut_ptr().cast(), 1) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
    assert_eq!(
        std::env::var("EXPLICIT_TEST_VALUE").unwrap(),
        "allowed-fake-value"
    );
    let base = PathBuf::from(std::env::var_os("EDGEE_TEST_BASE").unwrap());
    std::fs::write("allowed", "okay").unwrap();
    assert_eq!(std::fs::read_to_string("allowed").unwrap(), "okay");
    assert!(std::fs::read(base.join("secret")).is_err());
    assert!(std::fs::read("../secret").is_err());
    assert!(std::fs::read("/etc/passwd").is_err());
    assert!(std::fs::write(base.join("outside"), "denied").is_err());
    symlink(base.join("secret"), "escape-file").unwrap();
    assert!(std::fs::read("escape-file").is_err());
    assert!(std::fs::write("escape-file", "denied").is_err());
    symlink(&base, "escape-dir").unwrap();
    assert!(std::fs::write("escape-dir/escaped", "denied").is_err());
    assert!(std::fs::write(".git/config", "denied").is_err());
    assert!(std::fs::rename(".git/config", ".git/moved-config").is_err());
    assert!(std::fs::remove_file(".git/config").is_err());
    assert!(std::fs::rename(".git", "renamed-git").is_err());
    assert!(std::fs::write(".mcp.json", "{}").is_err());
    assert!(std::fs::create_dir(".claude").is_err());
    assert!(std::fs::create_dir(".git/hooks").is_err());
    for name in [
        "nested/.git/config",
        "nested/.vscode/settings.json",
        ".git/modules/subrepo/config",
    ] {
        assert!(
            std::fs::write(name, "denied").is_err(),
            "Nested security config {name}"
        );
    }

    assert!(ProcessCommand::new("/bin/sh")
        .args(["-c", "printf local-mcp > mcp-output"])
        .status()
        .unwrap()
        .success());
    let denied = ProcessCommand::new("/bin/sh")
        .args(["-c", "cat \"$1\"", "mcp"])
        .arg(base.join("secret"))
        .output()
        .unwrap();
    assert!(!denied.status.success());
    let endpoint = std::env::var("EDGEE_TEST_ENDPOINT").unwrap();
    assert!(
        TcpStream::connect_timeout(&endpoint.parse().unwrap(), Duration::from_secs(2)).is_err(),
        "Direct localhost bypass"
    );
    assert!(
        UnixStream::connect(std::env::var_os("EDGEE_TEST_SOCKET").unwrap()).is_err(),
        "Host Unix socket bypass"
    );
    let allowed = proxy_request(&endpoint, true);
    assert!(
        allowed.starts_with("HTTP/1.1 200") || allowed.starts_with("HTTP/1.0 200"),
        "Controlled proxy request: {allowed}"
    );
    assert!(allowed.ends_with("controlled-relay"));
    let unauthenticated = proxy_request(&endpoint, false);
    assert!(
        !unauthenticated.starts_with("HTTP/1.1 200")
            && !unauthenticated.starts_with("HTTP/1.0 200")
    );
    let forbidden = proxy_request("example.invalid:80", true);
    assert!(!forbidden.starts_with("HTTP/1.1 200") && !forbidden.starts_with("HTTP/1.0 200"));
    #[cfg(target_os = "macos")]
    {
        let output = ProcessCommand::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-a",
                "edgee-sandbox-test",
                "-s",
                "edgee-fake-secret",
                "-w",
            ])
            .arg(std::env::var_os("EDGEE_TEST_KEYCHAIN").unwrap())
            .output()
            .unwrap();
        assert!(!output.status.success(), "Keychain broker was accessible");
    }
}

fn proxy_request(endpoint: &str, authenticated: bool) -> String {
    let proxy = reqwest::Url::parse(&std::env::var("HTTP_PROXY").unwrap()).unwrap();
    let address = format!("{}:{}", proxy.host_str().unwrap(), proxy.port().unwrap());
    let mut stream =
        TcpStream::connect_timeout(&address.parse().unwrap(), Duration::from_secs(2)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let authorization = if authenticated {
        let token = format!(
            "{}:{}",
            proxy.username(),
            proxy.password().unwrap_or_default()
        );
        format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64::engine::general_purpose::STANDARD.encode(token)
        )
    } else {
        String::new()
    };
    write!(stream, "GET http://{endpoint}/ HTTP/1.1\r\nHost: {endpoint}\r\n{authorization}Connection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn native_sandbox_lifecycle_preserves_argv_exit_and_cleans_descendants() {
    let fixture = Fixture::new();
    let spec = fixture.spec();
    let mut command = fixture.command("/bin/sh");
    command.args([
        "-c",
        r#"
        test "$1" = 'spaces and "quotes"' || exit 90
        test "$2" = '$(touch never-evaluated)' || exit 91
        (/bin/sleep 2; printf leaked > surviving-descendant) &
        exit 42
    "#,
        "edgee-test",
        "spaces and \"quotes\"",
        "$(touch never-evaluated)",
    ]);
    assert_eq!(execute(&command, &spec).unwrap().code(), Some(42));
    thread::sleep(Duration::from_secs(3));
    assert!(!fixture.workspace.join("surviving-descendant").exists());
    assert!(!fixture.workspace.join("never-evaluated").exists());
    let ready = fixture.workspace.join("signal-ready");
    let sender = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "Signal fixture did not start");
            thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }
    });
    let mut interrupted = fixture.command("/bin/sh");
    interrupted.args([
        "-c",
        "touch signal-ready; (/bin/sleep 3; touch signal-descendant-survived) & /bin/sleep 30",
    ]);
    let status = execute(&interrupted, &spec).unwrap();
    sender.join().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
    thread::sleep(Duration::from_secs(4));
    assert!(!fixture
        .workspace
        .join("signal-descendant-survived")
        .exists());
}

fn host_git(cwd: &Path, args: &[&str]) -> String {
    let output = ProcessCommand::new(native_git().unwrap())
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
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn native_sandbox_linked_worktree_commits_without_configuration_writes() {
    let fixture = Fixture::new();
    let main = fixture.base.join("main");
    std::fs::create_dir(&main).unwrap();
    host_git(&main, &["init", "-b", "main"]);
    std::fs::write(main.join("file"), "before\n").unwrap();
    host_git(&main, &["add", "file"]);
    host_git(&main, &["commit", "-m", "initial"]);
    host_git(
        &main,
        &[
            "worktree",
            "add",
            "-b",
            "sandbox",
            fixture.workspace.to_str().unwrap(),
        ],
    );
    let common = main.join(".git").canonicalize().unwrap();
    let gitdir = PathBuf::from(host_git(
        &fixture.workspace,
        &["rev-parse", "--absolute-git-dir"],
    ))
    .canonicalize()
    .unwrap();
    std::fs::create_dir(gitdir.join("hooks")).unwrap();
    std::fs::write(gitdir.join("hooks/pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
    let prepared = policy::Prepared {
        workspace: fixture.workspace.clone(),
        cwd: fixture.workspace.clone(),
        policy: policy::Policy::default(),
        git_metadata: vec![gitdir.clone(), common.clone()],
        clone_url: None,
    };
    let (writes, denies) = worktree_access(&prepared).unwrap();
    let mut spec = fixture.spec();
    spec.read_paths.extend(writes.clone());
    for name in [
        "config",
        "HEAD",
        "packed-refs",
        "shallow",
        "description",
        "info/exclude",
        "info/attributes",
    ] {
        let candidate = common.join(name);
        if candidate.exists() {
            spec.read_paths.push(candidate.canonicalize().unwrap());
        }
    }
    spec.write_paths = writes;
    spec.deny_write_paths.extend(denies);
    let mut command = fixture.command("/bin/sh");
    command.env("GIT_AUTHOR_NAME", "Sandbox Test").env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Sandbox Test").env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .args(["-c", r#"
        set -eu
        for file in "$2/config" "$2/hooks/pre-commit" "$3/hooks/pre-commit" "$3/commondir" "$3/gitdir" "$2/objects/info/alternates" "$4/file"; do
            if (printf forbidden > "$file") 2>/dev/null; then exit 90; fi
        done
        printf 'after\n' > file
        "$1" status --porcelain
        "$1" add file
        "$1" commit -m 'sandbox commit'
        "#, "worktree-test"])
        .arg(native_git().unwrap()).arg(&common).arg(&gitdir).arg(&main);
    assert!(execute(&command, &spec).unwrap().success());
    assert_eq!(
        host_git(&fixture.workspace, &["log", "-1", "--format=%s"]),
        "sandbox commit"
    );
    assert_eq!(
        std::fs::read_to_string(main.join("file")).unwrap(),
        "before\n"
    );
}

#[test]
fn native_sandbox_https_repository_clone() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec();
    spec.allowed_domains = vec!["github.com".into()];
    spec.cwd = fixture.session.clone();
    spec.deny_write_paths.clear();
    let template = fixture.session.join("git-template");
    std::fs::create_dir(&template).unwrap();
    let mut command = fixture.command(native_git().unwrap());
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "http.followRedirects=false",
            "-c",
            "core.hooksPath=/dev/null",
            "clone",
            "--depth=1",
            "--template",
        ])
        .arg(template)
        .args(["--", "https://github.com/edgee-ai/edgee.git"])
        .arg(&fixture.workspace);
    assert!(execute(&command, &spec).unwrap().success());
    assert!(fixture.workspace.join("README.md").is_file());
    assert!(fixture.workspace.join(".git").is_dir());
    assert!(!fixture.workspace.join(".git/hooks/post-checkout").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn native_sandbox_missing_helper_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    assert!(preflight(directory.path().to_str().unwrap()).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn native_sandbox_internal_adapter_refuses_direct_invocation() {
    let directory = tempfile::tempdir().unwrap();
    let payload = directory.path().join("not-a-helper-payload.json");
    std::fs::write(&payload, "{}").unwrap();
    let binary =
        std::env::var_os("EDGEE_TEST_BINARY").expect("Build edgee and set EDGEE_TEST_BINARY");
    let output = ProcessCommand::new(binary)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .arg("sandbox-internal")
        .arg(&payload)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires a prepared PID namespace"));
}
