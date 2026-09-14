//! `edgee launch claude-desktop` — Claude Desktop's **Claude Code** through Edgee.
//!
//! Thin delegate to [`crate::commands::relay`], which MITMs `api.anthropic.com` with
//! a CA name-constrained to `anthropic.com` and trusted in the macOS System keychain
//! (Chromium consults only the OS store).
//!
//! **Scope: Claude Code, not the app's chat.** Claude Desktop's own chat POSTs
//! `claude.ai/api/organizations/{org}/chat_conversations/{uuid}/completion` — a
//! different registrable domain, absent from `INFERENCE_HOSTS`, so the relay
//! blind-tunnels it and never decrypts it. Those turns bill the user's Claude plan
//! directly and are invisible to Edgee. Verified by packet capture (2026-08-26,
//! Claude Desktop 1.37937.1): a full chat exchange produced that one `completion`
//! POST and **zero** `/v1/messages`, the only Anthropic path the relay reroutes.
//!
//! Widening this is not a host-list edit. The CA is a *persistent, machine-wide*
//! trust root, constrained to `anthropic.com` precisely so a leak could vouch for
//! nothing else (see `relay::ensure_claude_desktop_ca`). Covering chat means trusting
//! it for `claude.ai` too — the domain hosting the user's entire Claude web session.
//! That is a security decision, not a config change.
//!
//! Structurally identical to [`super::codex_desktop`]: both vendors' chat clients talk
//! to the consumer web backend rather than the API host the gateway knows.

#[cfg(target_os = "windows")]
use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
pub struct Options {}

pub async fn run(_opts: Options) -> Result<()> {
    crate::commands::relay::run_for_agent("claude-desktop").await
}

/// Shared by the relay launcher and desktop-alias detection.
#[cfg(target_os = "windows")]
pub(crate) fn windows_binary() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("LOCALAPPDATA")
            .map(|a| PathBuf::from(a).join("AnthropicClaude").join("claude.exe")),
        std::env::var_os("PROGRAMFILES")
            .map(|a| PathBuf::from(a).join("Claude").join("claude.exe")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
        .or_else(windows_msix_binary)
}

/// Ask Windows for the current user's registered package and its manifest's
/// executable, avoiding hardcoded versions or enumerating protected WindowsApps.
/// This only resolves the path; the relay still launches the executable directly.
#[cfg(target_os = "windows")]
fn windows_msix_binary() -> Option<PathBuf> {
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Get-AppxPackage -Name Claude | Sort-Object -Property Version -Descending | ForEach-Object {
    $package = $_
    $manifest = Get-AppxPackageManifest -Package $package.PackageFullName
    foreach ($app in $manifest.Package.Applications.Application) {
        $executable = [string]$app.Executable
        if ($executable -and [System.IO.Path]::GetFileName($executable) -ieq 'claude.exe') {
            Join-Path -Path $package.InstallLocation -ChildPath $executable
        }
    }
}
"#,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|path| path.is_file())
}
