pub fn resolve_binary(name: &str) -> std::ffi::OsString {
    #[cfg(not(windows))]
    {
        name.into()
    }

    #[cfg(windows)]
    {
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
