// Resolve the agent binary to launch. On unix we search `PATH` ourselves so we
// can skip Edgee's own shim directory (`~/.edgee/bin`, installed by
// `edgee alias`): the shims are named after the agents and re-run
// `edgee launch <agent>`, which would re-enter this process and drop the
// original flags (notably `--profile`) and env. We only skip the shim dir for
// *resolution* — the process env is untouched, so the spawned agent still
// inherits the full `PATH` and nested alias shims keep working.
//
// Windows aliases are shell aliases, not `PATH` shims (`USES_SHIMS = cfg!(unix)`
// in `commands::alias`), so the re-entry can't happen there; that branch keeps
// its original `which`/npm resolution.
#[cfg(not(windows))]
pub fn resolve_binary(name: &str) -> std::ffi::OsString {
    let Some(path) = std::env::var_os("PATH") else {
        return name.into();
    };
    let shim_dir = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| std::path::Path::new(&home).join(".edgee/bin"));

    find_on_path(name, &path, shim_dir.as_deref())
        .map(std::path::PathBuf::into_os_string)
        .unwrap_or_else(|| name.into())
}

/// First executable named `name` across `path`, skipping `shim_dir`. `None` if
/// none is found (caller falls back to bare `name` and lets the OS resolve it).
#[cfg(not(windows))]
fn find_on_path(
    name: &str,
    path: &std::ffi::OsStr,
    shim_dir: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    // A name with a path component is not PATH-searched (exec semantics).
    if name.is_empty() || name.contains('/') {
        return None;
    }

    for dir in std::env::split_paths(path) {
        if shim_dir == Some(dir.as_path()) {
            continue;
        }
        let candidate = dir.join(name);
        match std::fs::metadata(&candidate) {
            Ok(meta) if meta.is_file() && meta.permissions().mode() & 0o111 != 0 => {
                return Some(candidate);
            }
            _ => {}
        }
    }
    None
}

#[cfg(windows)]
pub fn resolve_binary(name: &str) -> std::ffi::OsString {
    if let Ok(found) = which::which(name) {
        return found.into_os_string();
    }

    if let Some(npm_bin) = npm_global_bin_dir() {
        for ext in &["cmd", "exe", "ps1"] {
            let candidate = npm_bin.join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return candidate.into_os_string();
            }
        }
    }

    name.into()
}

#[cfg(windows)]
fn npm_global_bin_dir() -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("npm")
        .args(["config", "get", "prefix"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let prefix = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if prefix.is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(prefix))
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::find_on_path;

    fn mkexec(dir: &Path, name: &str) {
        let p = dir.join(name);
        fs::write(&p, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn join(paths: &[&Path]) -> OsString {
        std::env::join_paths(paths.iter().map(|p| p.as_os_str())).unwrap()
    }

    #[test]
    fn skips_shim_dir_and_finds_real_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("shim");
        let real = tmp.path().join("real");
        fs::create_dir(&shim).unwrap();
        fs::create_dir(&real).unwrap();
        mkexec(&shim, "claude");
        mkexec(&real, "claude");

        // Shim dir comes first on PATH but must be skipped.
        let path = join(&[shim.as_path(), real.as_path()]);
        let got = find_on_path("claude", &path, Some(shim.as_path())).unwrap();
        assert_eq!(got, real.join("claude"));
    }

    #[test]
    fn returns_none_when_only_shim_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("shim");
        fs::create_dir(&shim).unwrap();
        mkexec(&shim, "claude");

        let path = join(&[shim.as_path()]);
        assert!(find_on_path("claude", &path, Some(shim.as_path())).is_none());
    }

    #[test]
    fn finds_binary_when_no_shim_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        mkexec(&bin, "codex");

        let path = join(&[bin.as_path()]);
        let got = find_on_path("codex", &path, None).unwrap();
        assert_eq!(got, bin.join("codex"));
    }

    #[test]
    fn ignores_non_executable_files() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("codex"), "x").unwrap(); // no +x bit

        let path = join(&[bin.as_path()]);
        assert!(find_on_path("codex", &path, None).is_none());
    }

    #[test]
    fn rejects_names_with_path_separator() {
        let tmp = tempfile::tempdir().unwrap();
        let path = join(&[tmp.path()]);
        assert!(find_on_path("a/b", &path, None).is_none());
    }
}
