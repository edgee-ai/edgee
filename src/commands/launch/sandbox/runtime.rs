//! Pinned Anthropic Sandbox Runtime supervisor. No implicit unconfined fallback.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{bail, Context, Result};
use serde::Serialize;

const DRIVER: &str = include_str!("runtime-driver.mjs");

#[derive(Debug, Serialize)]
pub struct RuntimeSpec {
    pub workspace: PathBuf,
    pub cwd: PathBuf,
    pub session_dir: PathBuf,
    pub read_paths: Vec<PathBuf>,
    pub write_paths: Vec<PathBuf>,
    pub deny_write_paths: Vec<PathBuf>,
    pub allowed_domains: Vec<String>,
    pub runtime_root: PathBuf,
    pub node_binary: PathBuf,
    pub tool_path: String,
}

/// Dependency validation runs before authentication or agent setup. Both paths
/// must already have been validated against the applicable administrator policy.
pub fn preflight(node_binary: &Path, runtime_root: &Path, tool_path: &str) -> Result<()> {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        bail!("Local sandbox supports macOS and Linux only; use a supported host.");
    }
    if !node_binary.is_absolute() || !runtime_root.is_absolute() {
        bail!("Sandbox Node.js and runtime paths must be absolute.");
    }
    let package = runtime_root.join("node_modules/@anthropic-ai/sandbox-runtime/package.json");
    if !node_binary.is_file() || !package.is_file() {
        bail!("Sandbox runtime is missing. Install Node.js >=22.12 and the pinned dependencies from scripts/sandbox-runtime with npm ci --ignore-scripts --omit=optional; configure their absolute paths.");
    }
    let payload = serde_json::json!({"preflight": true, "runtime_root": runtime_root});
    let mut input =
        tempfile::NamedTempFile::new().context("Create private sandbox preflight input")?;
    serde_json::to_writer(&mut input, &payload)?;
    input.flush()?;
    let mut cmd = supervisor(node_binary, input.path(), tool_path);
    let status = cmd
        .status()
        .context("Start sandbox dependency check; verify the configured Node.js binary")?;
    if !status.success() {
        bail!("Sandbox dependency check failed; install the required runtime dependencies. No agent was launched.");
    }
    Ok(())
}

fn supervisor(node: &Path, input: &Path, tool_path: &str) -> Command {
    let mut command = Command::new(node);
    command.env_clear().env("PATH", tool_path);
    command.args(["--input-type=module", "--eval", DRIVER]);
    command.arg(input);
    command
}

/// Run only an explicitly constructed child environment. Inherited variables
/// are intentionally ignored even if the caller forgot Command::env_clear().
pub fn execute(command: &Command, spec: &RuntimeSpec) -> Result<ExitStatus> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (command, spec);
        bail!("Local sandbox supports macOS and Linux only; use a supported host.");
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        execute_unix(command, spec)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn execute_unix(command: &Command, spec: &RuntimeSpec) -> Result<ExitStatus> {
    use std::collections::BTreeMap;
    use std::os::unix::process::CommandExt;

    let utf8 = |value: &std::ffi::OsStr| -> Result<String> {
        Ok(value
            .to_str()
            .context("Sandbox arguments and environment must be valid UTF-8")?
            .to_owned())
    };
    let argv: Vec<String> = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(utf8)
        .collect::<Result<_>>()?;
    let env: BTreeMap<String, String> = command
        .get_envs()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .map(|(key, value)| Ok((utf8(key)?, utf8(value)?)))
        .collect::<Result<_>>()?;
    let mut payload = serde_json::to_value(spec)?;
    payload["argv"] = serde_json::to_value(argv)?;
    payload["env"] = serde_json::to_value(env)?;
    // Outside the session directory: the sandbox must never read the launch
    // specification. NamedTempFile uses 0600; the supervisor unlinks it at once.
    let mut input =
        tempfile::NamedTempFile::new().context("Create private sandbox launch input")?;
    serde_json::to_writer(&mut input, &payload)?;
    input.flush()?;
    let signals = ExecutionSignals::new()?;
    let mut supervisor = supervisor(&spec.node_binary, input.path(), &spec.tool_path);
    supervisor
        .current_dir(&spec.cwd)
        .env("HOME", &spec.session_dir);
    supervisor.process_group(0);
    let mut child = supervisor
        .spawn()
        .context("Start OS sandbox supervisor; agent was not launched")?;
    let guard = ProcessGroupGuard::new(child.id() as libc::pid_t)?;
    let mut forwarded = None;
    let mut deadline = None;
    let status = loop {
        let signal = signals.pending.swap(0, std::sync::atomic::Ordering::SeqCst) as libc::c_int;
        if signal != 0 {
            // Terminal signals normally reach the foreground child directly.
            // Signals explicitly sent to Edgee (including noninteractive jobs)
            // are forwarded instead of terminating Rust before session cleanup.
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), signal);
            }
            forwarded = Some(signal);
            deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
        }
        if deadline.is_some_and(|time| std::time::Instant::now() >= time) {
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            deadline = None;
        }
        if let Some(status) = child.try_wait().context("Wait for OS sandbox supervisor")? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    drop(guard);
    if let Some(signal) = forwarded {
        use std::os::unix::process::ExitStatusExt;
        return Ok(ExitStatus::from_raw(signal));
    }
    Ok(status)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct ExecutionSignals {
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    registrations: Vec<signal_hook::SigId>,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl ExecutionSignals {
    fn new() -> Result<Self> {
        let mut guard = Self {
            pending: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            registrations: Vec::new(),
        };
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            guard.registrations.push(
                signal_hook::flag::register_usize(
                    signal,
                    std::sync::Arc::clone(&guard.pending),
                    signal as usize,
                )
                .context("Install sandbox signal forwarding; launch refused")?,
            );
        }
        Ok(guard)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for ExecutionSignals {
    fn drop(&mut self) {
        // Remove only our registrations, preserving existing Tokio/auth handlers.
        for registration in &self.registrations {
            signal_hook::low_level::unregister(*registration);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct ProcessGroupGuard {
    group: libc::pid_t,
    previous: libc::pid_t,
    terminal: bool,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl ProcessGroupGuard {
    fn new(group: libc::pid_t) -> Result<Self> {
        // SAFETY: libc receives only valid process IDs and the inherited stdin
        // descriptor; tcgetpgrp reports ENOTTY for pipes/noninteractive input.
        let previous = unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) };
        let terminal = previous >= 0 && previous == unsafe { libc::getpgrp() };
        let guard = Self {
            group,
            previous,
            terminal,
        };
        if terminal && !set_foreground(group) {
            bail!("Cannot give the sandbox control of the terminal; launch refused.");
        }
        Ok(guard)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        // Kill surviving descendants before removing their workspace/session.
        // Linux additionally confines all descendants to a PID namespace.
        unsafe {
            libc::kill(-self.group, libc::SIGKILL);
        }
        if self.terminal {
            set_foreground(self.previous);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn set_foreground(group: libc::pid_t) -> bool {
    // Block SIGTTOU only on this thread while changing the foreground process
    // group. Installing a process-global signal handler would race other code.
    unsafe {
        let mut blocked: libc::sigset_t = std::mem::zeroed();
        let mut previous: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGTTOU);
        if libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous) != 0 {
            return false;
        }
        let success = libc::tcsetpgrp(libc::STDIN_FILENO, group) == 0;
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        success
    }
}

// This suite is explicitly enabled in the macOS/Linux isolation CI job. No
// missing-runtime skip: unavailable dependencies or missing fixtures fail it.
#[cfg(all(test, feature = "sandbox-runtime-tests", unix))]
mod real_runtime_tests {
    use super::*;

    #[test]
    fn real_runtime_lifecycle_filters_inherited_secrets_and_cleans_descendants() {
        assert_eq!(
            std::env::var("FAKE_HOST_SECRET").as_deref(),
            Ok("host-only-fake-secret"),
            "Set FAKE_HOST_SECRET=host-only-fake-secret in the test parent environment"
        );
        let node_binary = PathBuf::from(
            std::env::var_os("EDGEE_TEST_NODE")
                .expect("EDGEE_TEST_NODE must name the installed Node.js binary"),
        )
        .canonicalize()
        .unwrap();
        let runtime_root = PathBuf::from(
            std::env::var_os("EDGEE_TEST_RUNTIME_ROOT")
                .expect("EDGEE_TEST_RUNTIME_ROOT must name the installed pinned runtime"),
        )
        .canonicalize()
        .unwrap();
        let tool_path = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_owned();
        preflight(&node_binary, &runtime_root, &tool_path)
            .expect("Real runtime preflight must succeed");
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let workspace = base.join("workspace with spaces");
        let session_dir = base.join("private session");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&session_dir).unwrap();
        let secret = base.join("fake-secret-outside");
        std::fs::write(&secret, "fake-file-secret").unwrap();
        let spec = RuntimeSpec {
            cwd: workspace.clone(),
            workspace: workspace.clone(),
            session_dir: session_dir.clone(),
            read_paths: super::super::system_read_paths(),
            write_paths: vec![],
            deny_write_paths: vec![],
            allowed_domains: vec![],
            runtime_root,
            node_binary,
            tool_path,
        };
        // Deliberately DON'T env_clear here. execute must omit inherited state
        // even when a future launcher forgets to scrub its Command explicitly.
        let mut command = Command::new("/bin/sh");
        command
            .env("HOME", &session_dir)
            .env("TMPDIR", &session_dir)
            .env("PATH", "/usr/bin:/bin")
            .env("EXPLICIT_TEST_VALUE", "allowed-fake-value")
            .args([
                "-c",
                r#"
                test -z "${FAKE_HOST_SECRET+x}" || exit 90
                test "$EXPLICIT_TEST_VALUE" = allowed-fake-value || exit 91
                test "$1" = 'spaces and "quotes"' || exit 92
                test "$2" = '$(touch never-evaluated)' || exit 93
                test ! -r "$3" || exit 94
                printf '%s' "$EXPLICIT_TEST_VALUE" > proof
                (/bin/sleep 2; printf leaked > surviving-descendant) &
                exit 42
            "#,
                "edgee-test",
                "spaces and \"quotes\"",
                "$(touch never-evaluated)",
            ])
            .arg(&secret);
        let status = execute(&command, &spec).expect("Execute through actual OS sandbox");
        assert_eq!(
            status.code(),
            Some(42),
            "Agent exit status must survive the Rust supervisor"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("proof")).unwrap(),
            "allowed-fake-value"
        );
        assert!(!workspace.join("never-evaluated").exists());
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert!(
            !workspace.join("surviving-descendant").exists(),
            "Rust process-group cleanup must stop background children"
        );
        assert_eq!(std::fs::read_to_string(secret).unwrap(), "fake-file-secret");

        // Signal the Rust parent, not its foreground child. This is what job
        // supervisors do for cancellation and must still return through cleanup.
        let ready = workspace.join("signal-ready");
        let signal_ready = ready.clone();
        let sender = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !signal_ready.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Signal fixture did not start"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            unsafe {
                libc::kill(libc::getpid(), libc::SIGTERM);
            }
        });
        let mut interrupted = Command::new("/bin/sh");
        interrupted.env_clear().env("HOME", &session_dir).env("TMPDIR", &session_dir)
            .env("PATH", "/usr/bin:/bin")
            .args(["-c", "touch signal-ready; (/bin/sleep 3; touch signal-descendant-survived) & /bin/sleep 30"]);
        let interrupted_status = execute(&interrupted, &spec).unwrap();
        sender.join().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(interrupted_status.signal(), Some(libc::SIGTERM));
        std::thread::sleep(std::time::Duration::from_secs(4));
        assert!(!workspace.join("signal-descendant-survived").exists());
    }
}
