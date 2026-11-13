//! Native OS sandbox orchestration. A requested sandbox never falls back.
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Stdio;
use std::process::{Command, ExitStatus};

#[cfg(unix)]
use anyhow::Context;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[cfg(any(target_os = "linux", all(test, unix)))]
#[path = "git_excludes.rs"]
mod git_excludes;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod linux;
#[cfg(unix)]
#[path = "proxy.rs"]
mod proxy;
#[cfg(target_os = "macos")]
#[path = "seatbelt.rs"]
mod seatbelt;
#[cfg(target_os = "linux")]
#[path = "seccomp.rs"]
mod seccomp;

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeSpec {
    pub workspace: PathBuf,
    pub cwd: PathBuf,
    pub session_dir: PathBuf,
    pub read_paths: Vec<PathBuf>,
    pub write_paths: Vec<PathBuf>,
    pub deny_write_paths: Vec<PathBuf>,
    pub allowed_domains: Vec<String>,
    pub tool_path: String,
}
#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct Payload {
    argv: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: PathBuf,
    socket: PathBuf,
}
pub fn preflight(tool_path: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = tool_path;
        anyhow::ensure!(Path::new("/usr/bin/sandbox-exec").is_file(), "macOS Seatbelt launcher is unavailable; repair the operating system before sandboxing.");
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        linux::bwrap(tool_path).map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = tool_path;
        bail!("Local sandbox supports macOS and Linux only; use a supported host.")
    }
}
pub fn execute(command: &Command, spec: &RuntimeSpec) -> Result<ExitStatus> {
    preflight(&spec.tool_path)?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        execute_unix(command, spec)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = command;
        bail!("Local sandbox requires macOS or Linux")
    }
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn execute_unix(command: &Command, spec: &RuntimeSpec) -> Result<ExitStatus> {
    let proxy = proxy::Proxy::start(&spec.allowed_domains)?;
    let control = tempfile::Builder::new()
        .prefix("edgee-sandbox-control-")
        .tempdir()?;
    let control_path = control.path().canonicalize()?;
    #[cfg(target_os = "linux")]
    let _bridge = linux::HostBridge::start(&control_path.join("proxy.sock"), proxy.port)?;
    #[cfg(target_os = "linux")]
    let stubs = linux::DenyStubs::prepare(spec)?;
    let utf8 = |s: &std::ffi::OsStr| -> Result<String> {
        Ok(s.to_str()
            .context("Sandbox arguments and environment must be UTF-8")?
            .to_owned())
    };
    let argv = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(utf8)
        .collect::<Result<Vec<_>>>()?;
    let mut env = command
        .get_envs()
        .filter_map(|(k, v)| v.map(|v| (k, v)))
        .map(|(k, v)| Ok((utf8(k)?, utf8(v)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    // Native resolvers do not need access to host DNS files for a numeric proxy.
    #[cfg(target_os = "macos")]
    let port = proxy.port;
    #[cfg(target_os = "linux")]
    let port = linux::BRIDGE_PORT;
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "http_proxy",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        env.insert(name.into(), proxy.url(port));
    }
    for name in ["NO_PROXY", "no_proxy"] {
        env.insert(name.into(), String::new());
    }
    env.insert("GIT_CONFIG_COUNT".into(), "1".into());
    env.insert("GIT_CONFIG_KEY_0".into(), "safe.directory".into());
    env.insert(
        "GIT_CONFIG_VALUE_0".into(),
        spec.workspace.to_string_lossy().into_owned(),
    );
    #[cfg(target_os = "linux")]
    let excludes = git_excludes::write(
        spec,
        &stubs.created_paths(),
        &env.get("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| spec.session_dir.clone()),
    )?;
    #[cfg(target_os = "linux")]
    if let Some(file) = &excludes {
        env.insert("GIT_CONFIG_COUNT".into(), "2".into());
        env.insert("GIT_CONFIG_KEY_1".into(), "core.excludesfile".into());
        env.insert(
            "GIT_CONFIG_VALUE_1".into(),
            file.path().to_string_lossy().into_owned(),
        );
    }
    let payload = Payload {
        argv,
        env,
        cwd: spec.cwd.clone(),
        socket: control_path.join("proxy.sock"),
    };
    let secret = control_path.join("probe-secret");
    std::fs::write(&secret, b"fake-edgee-secret")?;
    let outside = control_path.join("probe-write");
    let probe = Payload {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            "test ! -r \"$1\" && ! (echo denied > \"$2\") 2>/dev/null".into(),
            "edgee-probe".into(),
            secret.to_string_lossy().into_owned(),
            outside.to_string_lossy().into_owned(),
        ],
        env: payload.env.clone(),
        cwd: payload.cwd.clone(),
        socket: payload.socket.clone(),
    };
    let mut probe_command = backend(&probe, spec, &control_path, proxy.port, true)?;
    probe_command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let status = supervise(probe_command, false).context("Start OS sandbox probe")?;
    anyhow::ensure!(status.success() && !outside.exists(), "OS sandbox probe failed; agent was not launched. Verify OS permissions and isolation dependencies.");
    eprintln!(
        "Edgee sandbox active ({}); workspace: {}",
        if cfg!(target_os = "macos") {
            "Seatbelt"
        } else {
            "bubblewrap + seccomp"
        },
        spec.workspace.display()
    );
    eprintln!(
        "Network allowlist: {}; host local services and Unix sockets blocked; isolated home.",
        spec.allowed_domains.join(", ")
    );
    let status = supervise(
        backend(&payload, spec, &control_path, proxy.port, false)?,
        true,
    )?;
    drop(proxy);
    Ok(status)
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn backend(
    payload: &Payload,
    spec: &RuntimeSpec,
    control: &Path,
    port: u16,
    probe: bool,
) -> Result<Command> {
    #[cfg(target_os = "macos")]
    {
        let _ = probe;
        seatbelt::command(payload, spec, control, port)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = port;
        linux::command(payload, spec, control, probe)
    }
}
#[cfg(target_os = "linux")]
pub(crate) fn internal(path: &Path) -> Result<()> {
    linux::internal(path)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn supervise(mut command: Command, foreground: bool) -> Result<ExitStatus> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let signals = ExecutionSignals::new()?;
    command.process_group(0);
    let mut child = command.spawn().context("Start native sandbox process")?;
    let guard = if foreground {
        ProcessGroupGuard::new(child.id() as libc::pid_t)?
    } else {
        ProcessGroupGuard {
            group: child.id() as libc::pid_t,
            previous: -1,
            terminal: false,
        }
    };
    let mut forwarded = None;
    let mut deadline = None;
    let status = loop {
        let signal = signals.pending.swap(0, std::sync::atomic::Ordering::SeqCst) as libc::c_int;
        if signal != 0 {
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
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    drop(guard);
    Ok(forwarded.map(ExitStatus::from_raw).unwrap_or(status))
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
