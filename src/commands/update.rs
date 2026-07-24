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
    // Homebrew-managed installs live under a read-only Cellar symlink, so the
    // self_update in-place atomic replace fails with `os error 2`. Detect those
    // and redirect to `brew upgrade` instead (EOSS-67 / SUPD-01, SUPD-02).
    //
    // Detection failure (no `current_exe`, or `canonicalize` error) falls through
    // to the existing direct-install flow so curl installs are never blocked.
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(real) = std::fs::canonicalize(&exe) {
            if is_homebrew_path(&real) {
                println!(
                    "edgee was installed via Homebrew. Run {} to upgrade.",
                    "brew upgrade edgee".cyan()
                );
                return Ok(());
            }
        }
    }

    // self_update uses synchronous reqwest client so we need to run it in a blocking task
    tokio::task::spawn_blocking(move || {
        use self_update::{backends::github::Update, Status};

        let updater = Update::configure()
            .repo_owner("edgee-ai")
            .repo_name("edgee")
            .bin_name("edgee")
            .current_version(self_update::cargo_crate_version!())
            .show_download_progress(true)
            .build()?;

        match updater.update()? {
            Status::Updated(version) => println!("Updated to {}", version.green()),
            Status::UpToDate(version) => println!("Already up to date ({})", version.green()),
        }

        Ok(())
    })
    .await?
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
