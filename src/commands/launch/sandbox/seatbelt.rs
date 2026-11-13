//! Generate a default-deny Seatbelt profile without an external runtime.
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::os::unix::process::CommandExt;

use anyhow::{Context, Result};

use super::{Payload, RuntimeSpec};

fn quoted(path: &Path) -> Result<String> {
    let text = path.to_str().context("Sandbox paths must be UTF-8")?;
    anyhow::ensure!(
        !text.chars().any(char::is_control),
        "Sandbox paths must not contain control characters"
    );
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

pub(super) fn command(
    payload: &Payload,
    spec: &RuntimeSpec,
    control: &Path,
    port: u16,
) -> Result<Command> {
    let mut profile = String::from(
        r#"(version 1)
(deny default)
(allow process-exec process-fork)
(allow process-info* (target same-sandbox))
(allow signal (target same-sandbox))
(allow mach-lookup
  (global-name "com.apple.system.logger")
  (global-name "com.apple.logd")
  (global-name "com.apple.system.notification_center")
  (global-name "com.apple.system.opendirectoryd.libinfo")
  (global-name "com.apple.system.opendirectoryd.membership")
  (global-name "com.apple.bsd.dirhelper"))
(allow sysctl-read
  (sysctl-name-prefix "hw.")
  (sysctl-name-prefix "machdep.cpu.")
  (sysctl-name-prefix "kern.os")
  (sysctl-name "kern.version")
  (sysctl-name "kern.bootargs")
  (sysctl-name "security.mac.lockdown_mode_state")
  (sysctl-name "kern.argmax")
  (sysctl-name "kern.hostname")
  (sysctl-name "kern.maxfilesperproc")
  (sysctl-name "kern.maxproc")
  (sysctl-name "kern.ngroups")
  (sysctl-name "kern.usrstack64")
  (sysctl-name "kern.tcsm_available")
  (sysctl-name "kern.tcsm_enable"))
(allow sysctl-write (sysctl-name "kern.tcsm_enable"))
(allow file-read-metadata (literal "/etc") (literal "/private/etc"))
(allow file-read-data (literal "/"))
(allow file-read* file-write-data file-ioctl
  (literal "/dev/null") (literal "/dev/zero")
  (literal "/dev/random") (literal "/dev/urandom") (literal "/dev/tty"))
(allow pseudo-tty)
(allow file-read* file-write-data file-ioctl
  (regex #"^/dev/ttys[0-9]+$") (literal "/dev/ptmx"))
"#,
    );
    profile.push_str(&format!(
        "(allow network-outbound (remote tcp \"localhost:{port}\"))\n"
    ));
    let mut ancestors = BTreeSet::new();
    for path in spec
        .read_paths
        .iter()
        .chain(&spec.write_paths)
        .chain([&spec.workspace, &spec.session_dir])
    {
        let filter = if path.is_dir() { "subpath" } else { "literal" };
        profile.push_str(&format!(
            "(allow file-read* ({filter} {}))\n",
            quoted(path)?
        ));
        for ancestor in path.ancestors().skip(1) {
            ancestors.insert(ancestor.to_path_buf());
        }
    }
    for ancestor in ancestors {
        profile.push_str(&format!(
            "(allow file-read-metadata (literal {}))\n",
            quoted(&ancestor)?
        ));
    }
    for path in spec
        .write_paths
        .iter()
        .chain([&spec.workspace, &spec.session_dir])
    {
        profile.push_str(&format!(
            "(allow file-write* (subpath {}))\n",
            quoted(path)?
        ));
    }
    // Denies also protect absent files and stop renaming their parent away to
    // replace a protected subtree. Canonical aliases cannot weaken the deny.
    for path in &spec.deny_write_paths {
        let mut paths = vec![path.clone()];
        if let Ok(canonical) = path.canonicalize() {
            if canonical != *path {
                paths.push(canonical);
            }
        }
        for path in paths {
            profile.push_str(&format!(
                "(deny file-write* (subpath {}))\n",
                quoted(&path)?
            ));
            for ancestor in path.ancestors().skip(1) {
                profile.push_str(&format!(
                    "(deny file-write-unlink (literal {}))\n",
                    quoted(ancestor)?
                ));
            }
        }
    }
    // Supervisor files are never available, even if the caller grants a broad
    // tool root. Workload credentials are supplied as env, not shell text.
    profile.push_str(&format!(
        "(deny file-read* file-write* (subpath {}))\n",
        quoted(control)?
    ));
    let program = payload.argv.first().context("Missing sandbox executable")?;
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command
        .env_clear()
        .envs(&payload.env)
        .current_dir(&payload.cwd)
        .args(["-p", &profile, "--", program])
        .args(&payload.argv[1..]);
    // An already-open host descriptor can carry access beyond path-based
    // policy. Keep only stdio across exec; preserve Rust's error pipe until
    // exec by setting CLOEXEC rather than closing descriptors in pre_exec.
    let descriptor_limit = unsafe { libc::getdtablesize() };
    unsafe {
        command.pre_exec(move || {
            for fd in 3..descriptor_limit {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 && libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_paths_escape_policy_syntax_and_reject_controls() {
        assert_eq!(
            quoted(Path::new("/tmp/a\"b\\c")).unwrap(),
            "\"/tmp/a\\\"b\\\\c\""
        );
        assert!(quoted(Path::new("/tmp/line\nbreak")).is_err());
    }
}
