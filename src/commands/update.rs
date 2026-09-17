use std::path::Path;

use colored::Colorize;

setup_command! {}

/// Classify an already-canonicalized executable path as Homebrew-managed.
///
/// Pure (no I/O): inspects only the passed-in path so it stays unit-testable.
/// Matches known Homebrew/Cellar location segments. Note `/usr/local/bin` must
/// NOT match — only the Cellar/Homebrew subpaths under `/usr/local` count.
fn is_homebrew_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/opt/homebrew/")
        || s.contains("/usr/local/Cellar/")
        || s.contains("/usr/local/Homebrew/")
        || s.contains("/home/linuxbrew/.linuxbrew/")
        || s.contains("/.linuxbrew/")
}

pub async fn run(_opts: Options) -> anyhow::Result<()> {
    perform_update(false, None).await?;
    Ok(())
}

pub(crate) fn self_update_disabled() -> bool {
    std::env::var_os("EDGEE_DISABLE_SELF_UPDATE").is_some()
}

pub(crate) fn installed_with_homebrew() -> bool {
    std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .is_ok_and(|path| is_homebrew_path(&path))
}

/// Returns true only when the executable was replaced. Launch-time updates
/// already have consent and pin the version shown in the prompt.
pub(crate) async fn perform_update(
    confirmed: bool,
    version: Option<String>,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        !self_update_disabled(),
        "Self-update is disabled by EDGEE_DISABLE_SELF_UPDATE. Contact your administrator to update Edgee."
    );
    // Resolve the fast-launch link target *before* any self-replace: once
    // self_update renames the new binary over the running one, Linux's
    // /proc/self/exe reads "<path> (deleted)" and canonicalize fails — baking
    // that bogus path into refreshed wrappers would break them.
    let launch_target = crate::commands::alias::edgee_executable().ok();

    // Homebrew-managed installs live under a read-only Cellar symlink, so the
    // self_update in-place atomic replace fails with `os error 2`. Detect those
    // and redirect to `brew upgrade` instead (EOSS-67 / SUPD-01, SUPD-02).
    //
    // Detection inspects the *canonical* path, not `launch_target`: the latter
    // may already be the stable `/usr/local/bin/edgee`, which is_homebrew_path
    // deliberately does not match.
    //
    // Detection failure (no `current_exe`, or `canonicalize` error) falls through
    // to the existing direct-install flow so curl installs are never blocked.
    if installed_with_homebrew() {
        // Stabilize any fast-launch links to the brew `bin/edgee` symlink
        // now, so they survive the deletion of the old Cellar version.
        refresh_launch_links(launch_target.as_deref());
        println!(
            "edgee was installed via Homebrew. Run {} to upgrade.",
            "brew upgrade edgee".cyan()
        );
        return Ok(false);
    }

    // self_update uses synchronous reqwest client so we need to run it in a blocking task
    let updated = tokio::task::spawn_blocking(move || {
        use self_update::{backends::github::Update, Status};

        let mut builder = Update::configure();
        builder
            .repo_owner("edgee-ai")
            .repo_name("edgee")
            .bin_name("edgee")
            .current_version(self_update::cargo_crate_version!())
            .show_download_progress(true)
            .no_confirm(confirmed);
        if let Some(version) = version {
            builder.target_version_tag(&format!("v{}", version.trim_start_matches('v')));
        }
        let updater = builder.build()?;

        let status = updater.update()?;
        let updated = matches!(status, Status::Updated(_));
        match status {
            Status::Updated(version) => println!("Updated to {}", version.green()),
            Status::UpToDate(version) => println!("Already up to date ({})", version.green()),
        }

        anyhow::Ok(updated)
    })
    .await??;

    // Desktop wrappers (`cursor`, `copilot`) bake in an absolute `edgee` path.
    // Refresh unconditionally: a wrapper can be stale even when the binary is
    // already current (e.g. baked against a previous install location).
    refresh_launch_links(launch_target.as_deref());

    Ok(updated)
}

/// Best-effort refresh of installed fast-launch links (desktop wrappers).
/// A failure here must never fail the update itself.
fn refresh_launch_links(edgee: Option<&Path>) {
    match edgee {
        Some(edgee) => crate::commands::alias::refresh_installed(edgee),
        None => eprintln!(
            "Note: could not refresh fast-launch links: failed to resolve the edgee binary path"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_silicon_cellar_is_homebrew() {
        assert!(is_homebrew_path(Path::new(
            "/opt/homebrew/Cellar/edgee/0.2.9/bin/edgee"
        )));
    }

    #[test]
    fn apple_silicon_prefix_is_homebrew() {
        assert!(is_homebrew_path(Path::new("/opt/homebrew/bin/edgee")));
    }

    #[test]
    fn intel_cellar_is_homebrew() {
        assert!(is_homebrew_path(Path::new(
            "/usr/local/Cellar/edgee/0.2.9/bin/edgee"
        )));
    }

    #[test]
    fn intel_homebrew_is_homebrew() {
        assert!(is_homebrew_path(Path::new("/usr/local/Homebrew/bin/edgee")));
    }

    #[test]
    fn linuxbrew_is_homebrew() {
        assert!(is_homebrew_path(Path::new(
            "/home/linuxbrew/.linuxbrew/bin/edgee"
        )));
    }

    #[test]
    fn usr_local_bin_is_not_homebrew() {
        assert!(!is_homebrew_path(Path::new("/usr/local/bin/edgee")));
    }

    #[test]
    fn tmp_is_not_homebrew() {
        assert!(!is_homebrew_path(Path::new("/tmp/edgee")));
    }

    #[test]
    fn cargo_bin_is_not_homebrew() {
        assert!(!is_homebrew_path(Path::new(
            "/Users/someone/.cargo/bin/edgee"
        )));
    }

    #[test]
    fn usr_bin_is_not_homebrew() {
        assert!(!is_homebrew_path(Path::new("/usr/bin/edgee")));
    }
}
