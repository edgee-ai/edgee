//! Desktop app wrappers installed by `edgee alias` for GUI launch targets
//! (`cursor`, `copilot-vscode`). Platform-native launchers:
//!   macOS  → `~/Applications/<Name> (Edgee).app`
//!   Linux  → `~/.local/share/applications/edgee-*.desktop`
//!   Windows → Start Menu `.lnk`
//!
//! Wrappers are only installed when the target app is already present.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use console::style;

/// A GUI launch target that gets a desktop wrapper (not a CLI PATH shim).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppSpec {
    /// Canonical `edgee launch` / `edgee alias` name (`cursor`, `copilot-vscode`).
    pub id: &'static str,
    /// Finder / app-menu label, e.g. `Cursor (Edgee)`.
    pub display_name: &'static str,
    /// Host product name used in skip messages (`Cursor`, `VS Code`).
    pub host_label: &'static str,
    /// Argument after `edgee launch`.
    pub launch_target: &'static str,
}

pub const CURSOR_APP: AppSpec = AppSpec {
    id: "cursor",
    display_name: "Cursor (Edgee)",
    host_label: "Cursor",
    launch_target: "cursor",
};

pub const COPILOT_VSCODE_APP: AppSpec = AppSpec {
    id: "copilot-vscode",
    display_name: "VS Code Copilot (Edgee)",
    host_label: "VS Code",
    launch_target: "copilot-vscode",
};

pub const ALL_APPS: &[AppSpec] = &[CURSOR_APP, COPILOT_VSCODE_APP];

#[derive(Clone, Copy)]
pub enum Action {
    Install,
    Remove,
}

/// Install or remove desktop wrappers for `apps`. Skips install when the
/// underlying editor is not detected (prints a dim note).
pub fn apply_apps(apps: &[AppSpec], action: Action) -> Result<()> {
    if apps.is_empty() {
        return Ok(());
    }

    let edgee = edgee_executable()?;

    for app in apps {
        match action {
            Action::Install => {
                if !target_app_installed(app) {
                    println!(
                        "  {} {} ({})",
                        style("skipped").yellow(),
                        app.id,
                        style(format!("{} not found — install the app first", app.host_label)).dim()
                    );
                    continue;
                }
                let path = install_wrapper(app, &edgee)?;
                println!(
                    "  {} {} ({})",
                    style("installed").green(),
                    path.display(),
                    style("app wrapper").dim()
                );
            }
            Action::Remove => {
                if let Some(path) = remove_wrapper(app)? {
                    println!(
                        "  {} {} ({})",
                        style("removed").green(),
                        path.display(),
                        style("app wrapper").dim()
                    );
                } else {
                    println!(
                        "  {} {} ({})",
                        style("unchanged").dim(),
                        app.id,
                        style("no app wrapper").dim()
                    );
                }
            }
        }
    }
    Ok(())
}

fn edgee_executable() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("resolving edgee executable path")?;
    // Prefer the canonical path so LaunchServices / .desktop get a stable target.
    Ok(exe.canonicalize().unwrap_or(exe))
}

/// True when the underlying editor (Cursor / VS Code) is installed.
pub fn target_app_installed(app: &AppSpec) -> bool {
    match app.id {
        "cursor" => cursor_installed(),
        "copilot-vscode" => vscode_installed(),
        _ => false,
    }
}

fn cursor_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_app_exists("Cursor.app")
    }
    #[cfg(target_os = "linux")]
    {
        which_exists("cursor")
            || desktop_file_exists("cursor.desktop")
            || Path::new("/opt/Cursor/cursor").is_file()
            || Path::new("/usr/share/cursor/cursor").is_file()
    }
    #[cfg(target_os = "windows")]
    {
        windows_cursor_exe().is_some()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

fn vscode_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_app_exists("Visual Studio Code.app") || macos_app_exists("Code - Insiders.app")
    }
    #[cfg(target_os = "linux")]
    {
        which_exists("code")
            || which_exists("code-insiders")
            || desktop_file_exists("code.desktop")
            || desktop_file_exists("code-insiders.desktop")
            || Path::new("/usr/share/code/code").is_file()
            || Path::new("/usr/share/code-insiders/code-insiders").is_file()
    }
    #[cfg(target_os = "windows")]
    {
        windows_vscode_exe().is_some()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
fn macos_app_exists(bundle_name: &str) -> bool {
    for base in applications_dirs() {
        if base.join(bundle_name).is_dir() {
            return true;
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn applications_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = home_dir() {
        dirs.push(home.join("Applications"));
    }
    dirs
}

#[cfg(target_os = "macos")]
fn find_macos_app(bundle_name: &str) -> Option<PathBuf> {
    for base in applications_dirs() {
        let p = base.join(bundle_name);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn which_exists(name: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {name} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn desktop_file_exists(name: &str) -> bool {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local/share/applications"));
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(xdg).join("applications"));
    }
    dirs.iter().any(|d| d.join(name).is_file())
}

#[cfg(target_os = "windows")]
fn windows_cursor_exe() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let candidate = local.join("Programs/cursor/Cursor.exe");
    candidate.is_file().then_some(candidate)
}

#[cfg(target_os = "windows")]
fn windows_vscode_exe() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    for rel in [
        "Programs/Microsoft VS Code/Code.exe",
        "Programs/Microsoft VS Code Insiders/Code - Insiders.exe",
    ] {
        let candidate = local.join(rel);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn install_wrapper(app: &AppSpec, edgee: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        install_macos_app(app, edgee)
    }
    #[cfg(target_os = "linux")]
    {
        install_linux_desktop(app, edgee)
    }
    #[cfg(target_os = "windows")]
    {
        install_windows_shortcut(app, edgee)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (app, edgee);
        anyhow::bail!("desktop wrappers are not supported on this platform")
    }
}

fn remove_wrapper(app: &AppSpec) -> Result<Option<PathBuf>> {
    #[cfg(target_os = "macos")]
    {
        remove_macos_app(app)
    }
    #[cfg(target_os = "linux")]
    {
        remove_linux_desktop(app)
    }
    #[cfg(target_os = "windows")]
    {
        remove_windows_shortcut(app)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app;
        Ok(None)
    }
}

// ─── macOS ───────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn macos_wrapper_path(app: &AppSpec) -> Result<PathBuf> {
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(home
        .join("Applications")
        .join(format!("{}.app", app.display_name)))
}

#[cfg(target_os = "macos")]
fn install_macos_app(app: &AppSpec, edgee: &Path) -> Result<PathBuf> {
    let app_path = macos_wrapper_path(app)?;
    let contents = app_path.join("Contents");
    let macos_dir = contents.join("MacOS");
    let resources = contents.join("Resources");
    std::fs::create_dir_all(&macos_dir)
        .with_context(|| format!("creating {}", macos_dir.display()))?;
    std::fs::create_dir_all(&resources)
        .with_context(|| format!("creating {}", resources.display()))?;

    let executable = macos_dir.join("edgee-launch");
    let script = macos_launcher_script(edgee, app.launch_target);
    write_executable(&executable, &script)?;

    let plist = macos_info_plist(app, "edgee-launch");
    std::fs::write(contents.join("Info.plist"), plist)
        .with_context(|| format!("writing Info.plist in {}", contents.display()))?;

    // Best-effort: reuse the host app's icon so the wrapper looks familiar.
    if let Some(src_icns) = macos_source_icns(app) {
        let dest = resources.join("AppIcon.icns");
        let _ = std::fs::copy(&src_icns, &dest);
    }

    // Refresh LaunchServices so Spotlight picks up the new/updated bundle.
    let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
        .args(["-f", &app_path.to_string_lossy()])
        .status();

    Ok(app_path)
}

#[cfg(target_os = "macos")]
fn macos_source_icns(app: &AppSpec) -> Option<PathBuf> {
    let bundle = match app.id {
        "cursor" => find_macos_app("Cursor.app")?,
        "copilot-vscode" => find_macos_app("Visual Studio Code.app")
            .or_else(|| find_macos_app("Code - Insiders.app"))?,
        _ => return None,
    };
    let resources = bundle.join("Contents/Resources");
    for name in ["Cursor.icns", "Code.icns", "app.icns", "electron.icns"] {
        let p = resources.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn remove_macos_app(app: &AppSpec) -> Result<Option<PathBuf>> {
    let app_path = macos_wrapper_path(app)?;
    if !app_path.exists() {
        return Ok(None);
    }
    std::fs::remove_dir_all(&app_path)
        .with_context(|| format!("removing {}", app_path.display()))?;
    Ok(Some(app_path))
}

/// Quote a string for safe use inside a single-quoted bash literal.
#[cfg(any(test, target_os = "macos"))]
fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Launcher that re-opens in Terminal.app when double-clicked (no TTY), so the
/// relay banner stays visible for the session.
#[cfg(any(test, target_os = "macos"))]
pub fn macos_launcher_script(edgee: &Path, launch_target: &str) -> String {
    let edgee_sh = sh_single_quote(&edgee.to_string_lossy());
    let target_sh = sh_single_quote(launch_target);
    // `launch_target` is a fixed identifier (no spaces); path may contain spaces.
    let edgee_as = edgee
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "\
#!/bin/bash\n\
# Edgee desktop wrapper — managed by `edgee alias`. Do not edit by hand.\n\
set -euo pipefail\n\
EDGEE={edgee_sh}\n\
TARGET={target_sh}\n\
# Double-click has no TTY — start the session inside Terminal.app.\n\
if [ ! -t 1 ]; then\n\
  osascript -e 'tell application \"Terminal\" to do script \"exec \\\"{edgee_as}\\\" launch {launch_target}\"'\n\
  osascript -e 'tell application \"Terminal\" to activate'\n\
  exit 0\n\
fi\n\
exec \"$EDGEE\" launch \"$TARGET\" \"$@\"\n"
    )
}

#[cfg(any(test, target_os = "macos"))]
pub fn macos_info_plist(app: &AppSpec, executable: &str) -> String {
    let bundle_id = format!("ai.edgee.launch.{}", app.id.replace('-', "."));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>{executable}</string>
	<key>CFBundleIdentifier</key>
	<string>{bundle_id}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>CFBundleDisplayName</key>
	<string>{name}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>1.0</string>
	<key>CFBundleVersion</key>
	<string>1</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
"#,
        name = app.display_name,
    )
}

#[cfg(target_os = "macos")]
fn write_executable(path: &Path, body: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(body.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.set_permissions(std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("installing {}", path.display()))?;
    Ok(())
}

// ─── Linux ───────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn linux_desktop_path(app: &AppSpec) -> Result<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".local/share")))
        .ok_or_else(|| anyhow::anyhow!("could not resolve XDG data home"))?;
    Ok(base
        .join("applications")
        .join(format!("edgee-{}.desktop", app.id)))
}

#[cfg(target_os = "linux")]
fn install_linux_desktop(app: &AppSpec, edgee: &Path) -> Result<PathBuf> {
    let path = linux_desktop_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = linux_desktop_entry(app, edgee);
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    let _ = Command::new("update-desktop-database")
        .arg(path.parent().unwrap())
        .status();
    Ok(path)
}

#[cfg(target_os = "linux")]
fn remove_linux_desktop(app: &AppSpec) -> Result<Option<PathBuf>> {
    let path = linux_desktop_path(app)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Some(path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn linux_desktop_entry(app: &AppSpec, edgee: &Path) -> String {
    // Terminal=true so the relay session stays visible.
    format!(
        "\
[Desktop Entry]\n\
Type=Application\n\
Version=1.0\n\
Name={name}\n\
Comment=Launch {name} through the Edgee gateway\n\
Exec=\"{edgee}\" launch {target}\n\
Terminal=true\n\
Categories=Development;IDE;\n\
StartupNotify=true\n\
# Managed by `edgee alias`. Do not edit by hand.\n\
X-Edgee-Managed=true\n\
",
        name = app.display_name,
        edgee = edgee.display(),
        target = app.launch_target,
    )
}

// ─── Windows ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn windows_shortcut_path(app: &AppSpec) -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("APPDATA is not set"))?;
    Ok(appdata
        .join("Microsoft/Windows/Start Menu/Programs/Edgee")
        .join(format!("{}.lnk", app.display_name)))
}

#[cfg(target_os = "windows")]
fn install_windows_shortcut(app: &AppSpec, edgee: &Path) -> Result<PathBuf> {
    let path = windows_shortcut_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let icon = match app.id {
        "cursor" => windows_cursor_exe(),
        "copilot-vscode" => windows_vscode_exe(),
        _ => None,
    };

    // PowerShell COM shortcut — avoids a Windows-only crate dependency.
    let edgee_s = edgee.to_string_lossy().replace('\'', "''");
    let path_s = path.to_string_lossy().replace('\'', "''");
    let args = format!("launch {}", app.launch_target);
    let icon_line = match icon {
        Some(i) => {
            let i = i.to_string_lossy().replace('\'', "''");
            format!("$s.IconLocation = '{i},0'\n")
        }
        None => String::new(),
    };
    let ps = format!(
        "$w = New-Object -ComObject WScript.Shell\n\
         $s = $w.CreateShortcut('{path_s}')\n\
         $s.TargetPath = '{edgee_s}'\n\
         $s.Arguments = '{args}'\n\
         {icon_line}\
         $s.Save()\n"
    );

    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .status()
        .context("running PowerShell to create Start Menu shortcut")?;
    if !status.success() {
        anyhow::bail!("PowerShell failed to create shortcut at {}", path.display());
    }
    Ok(path)
}

#[cfg(target_os = "windows")]
fn remove_windows_shortcut(app: &AppSpec) -> Result<Option<PathBuf>> {
    let path = windows_shortcut_path(app)?;
    match std::fs::remove_file(&path) {
        Ok(()) => {
            // Best-effort: prune empty Edgee Start Menu folder.
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            Ok(Some(path))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_desktop_entry_points_at_edgee_launch() {
        let body = linux_desktop_entry(&CURSOR_APP, Path::new("/usr/local/bin/edgee"));
        assert!(body.contains("Name=Cursor (Edgee)"));
        assert!(body.contains("Exec=\"/usr/local/bin/edgee\" launch cursor"));
        assert!(body.contains("Terminal=true"));
        assert!(body.contains("X-Edgee-Managed=true"));
    }

    #[test]
    fn macos_info_plist_has_bundle_id_and_name() {
        let plist = macos_info_plist(&COPILOT_VSCODE_APP, "edgee-launch");
        assert!(plist.contains("ai.edgee.launch.copilot.vscode"));
        assert!(plist.contains("VS Code Copilot (Edgee)"));
        assert!(plist.contains("<string>edgee-launch</string>"));
    }

    #[test]
    fn macos_launcher_script_execs_edgee_launch() {
        let script = macos_launcher_script(Path::new("/opt/edgee"), "cursor");
        assert!(script.contains("#!/bin/bash"));
        assert!(script.contains("launch"));
        assert!(script.contains("cursor"));
        assert!(script.contains("/opt/edgee"));
        assert!(script.contains("Terminal"));
    }
}
