//! Shared plumbing for launch targets that load the `pi-edgee` extension.
//!
//! Two parts:
//!
//! - The launch context handed to the extension through a child-only env var.
//!   Pi and OMP use the same wire format; OMP marks itself with `agent`.
//! - Fetching an npm package into an Edgee-owned cache. OMP's `-e` takes
//!   file or directory paths only (no `npm:` specs like Pi), so the CLI
//!   downloads the pinned release from the registry, verifies its integrity
//!   hash, and unpacks it under `~/.edgee/extensions/`. Nothing is written to
//!   the agent's own directories.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};

/// Child-only env var carrying the [`LaunchContext`] JSON.
pub const CONTEXT_ENV: &str = "EDGEE_PI_CONTEXT";

/// `version` written into the context and the minimum `edgee.cliContract` a
/// user-installed pi-edgee must advertise in its `package.json`.
pub const CONTRACT_VERSION: u64 = 1;

const REGISTRY: &str = "https://registry.npmjs.org";

/// Packages are small (tens of KB); this guards against a misbehaving mirror.
const MAX_TARBALL_BYTES: usize = 8 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 32 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Everything pi-edgee needs to act as the CLI-selected identity. No `Debug`:
/// it holds credentials.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchContext {
    pub version: u64,
    pub session_id: String,
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_id: Option<String>,
    pub user_token: String,
    pub org_id: String,
    pub org_slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    pub gateway_url: String,
    pub console_url: String,
    pub console_api_url: String,
    pub mcp_url: String,
    pub mcp_disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_headers: Option<DebugHeaders>,
    /// Set for agents other than Pi. `"omp"` puts the extension in companion
    /// mode: the CLI owns the provider, the extension adds statusline and
    /// session metadata only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugHeaders {
    pub pubkey: String,
    pub salt: String,
}

/// Root of the extension cache (`~/.edgee/extensions`).
fn cache_root() -> Option<PathBuf> {
    Some(crate::config::edgee_home()?.join("extensions"))
}

/// Where `name@version` lives once fetched. A complete unpack always has a
/// `package.json`, which doubles as the "already fetched" marker.
fn package_dir(root: &Path, name: &str, version: &str) -> PathBuf {
    root.join(name).join(version)
}

#[derive(Deserialize)]
struct VersionMetadata {
    dist: Dist,
}

#[derive(Deserialize)]
struct Dist {
    tarball: String,
    integrity: Option<String>,
}

/// Returns the directory holding `name@version`, downloading it on first use.
pub async fn ensure_npm_package(name: &str, version: &str) -> Result<PathBuf> {
    let root = cache_root().context("Could not determine your home directory")?;
    let dir = package_dir(&root, name, version);
    if dir.join("package.json").is_file() {
        return Ok(dir);
    }

    let http = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .context("Failed to create HTTP client")?;
    let metadata: VersionMetadata = http
        .get(format!("{REGISTRY}/{name}/{version}"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("Failed to look up {name}@{version} on the npm registry"))?
        .json()
        .await
        .context("Unexpected response from the npm registry")?;
    let integrity = metadata
        .dist
        .integrity
        .as_deref()
        .with_context(|| format!("{name}@{version} has no integrity hash on the registry"))?;
    if !metadata.dist.tarball.starts_with("https://") {
        bail!("{name}@{version} points at a non-HTTPS tarball");
    }

    let bytes = http
        .get(&metadata.dist.tarball)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("Failed to download {name}@{version}"))?
        .bytes()
        .await
        .with_context(|| format!("Failed to download {name}@{version}"))?;
    if bytes.len() > MAX_TARBALL_BYTES {
        bail!("{name}@{version} is unexpectedly large, refusing to unpack it");
    }
    verify_integrity(&bytes, integrity)?;

    // Unpack beside the target and rename, so an interrupted run or a
    // concurrent launch never sees a half-written directory.
    let parent = dir.parent().context("Invalid extension cache path")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create {}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".unpack-")
        .tempdir_in(parent)
        .context("Failed to create a staging directory")?;
    unpack_package(&bytes, staging.path())?;
    match std::fs::rename(staging.path(), &dir) {
        Ok(()) => {
            // The directory now lives at `dir`; stop the guard deleting it.
            let _ = staging.keep();
        }
        // A concurrent launch won the race; its copy is equivalent.
        Err(_) if dir.join("package.json").is_file() => {}
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to install {}", dir.display()));
        }
    }
    Ok(dir)
}

/// Checks `bytes` against an npm `sha512-<base64>` integrity string. Weaker
/// algorithms are rejected rather than silently accepted.
fn verify_integrity(bytes: &[u8], integrity: &str) -> Result<()> {
    let expected = integrity
        .split_whitespace()
        .find_map(|entry| entry.strip_prefix("sha512-"))
        .context("The registry integrity hash is not sha512")?;
    let actual = base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes));
    if actual != expected {
        bail!("Integrity check failed for the downloaded package");
    }
    Ok(())
}

/// Unpacks an npm tarball (every entry under a single `package/` prefix) into
/// `dest`. Only regular files and directories are written, and any path that
/// would leave `dest` is rejected.
fn unpack_package(tarball: &[u8], dest: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(tarball);
    let mut archive = tar::Archive::new(decoder.take(MAX_UNPACKED_BYTES));
    for entry in archive.entries().context("Invalid package archive")? {
        let mut entry = entry.context("Invalid package archive entry")?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        let path = entry.path().context("Invalid path in package archive")?;
        let mut parts = path.components();
        // npm always nests everything under one top-level directory.
        parts.next();
        let relative: PathBuf = parts.collect();
        if relative.as_os_str().is_empty() {
            continue;
        }
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!("Package archive contains an unsafe path");
        }
        let target = dest.join(&relative);
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)
            .with_context(|| format!("Failed to write {}", target.display()))?;
        std::io::copy(&mut entry, &mut out)?;
    }
    if !dest.join("package.json").is_file() {
        bail!("The package archive has no package.json");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, content.as_bytes())
                .unwrap();
        }
        let raw = builder.into_inner().unwrap();
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &raw).unwrap();
        encoder.finish().unwrap()
    }

    fn integrity_of(bytes: &[u8]) -> String {
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
        )
    }

    fn sample_context(debug: bool, agent: Option<&'static str>) -> LaunchContext {
        LaunchContext {
            version: CONTRACT_VERSION,
            session_id: "42999158-ae9f-5b44-8834-27675aacf427".into(),
            api_key: "ek_test".into(),
            api_key_id: None,
            user_token: "tok_test".into(),
            org_id: "org-1".into(),
            org_slug: "acme".into(),
            org_name: Some("Acme".into()),
            gateway_url: "https://api.edgee.ai".into(),
            console_url: "https://www.edgee.ai".into(),
            console_api_url: "https://api.edgee.app".into(),
            mcp_url: "https://api.edgee.app/mcp".into(),
            mcp_disabled: false,
            debug_headers: debug.then(|| DebugHeaders {
                pubkey: "pk".into(),
                salt: "salt".into(),
            }),
            agent,
        }
    }

    #[test]
    fn context_uses_the_camel_case_wire_format() {
        let json = serde_json::to_value(sample_context(true, None)).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["sessionId"], "42999158-ae9f-5b44-8834-27675aacf427");
        assert_eq!(json["apiKey"], "ek_test");
        assert_eq!(json["userToken"], "tok_test");
        assert_eq!(json["consoleApiUrl"], "https://api.edgee.app");
        assert_eq!(json["mcpDisabled"], false);
        assert_eq!(json["debugHeaders"]["pubkey"], "pk");
    }

    #[test]
    fn context_omits_unset_optionals() {
        let json = serde_json::to_value(sample_context(false, None)).unwrap();
        assert!(json.get("debugHeaders").is_none());
        assert!(json.get("apiKeyId").is_none());
        assert!(json.get("agent").is_none());
    }

    #[test]
    fn context_marks_the_omp_companion() {
        let json = serde_json::to_value(sample_context(false, Some("omp"))).unwrap();
        assert_eq!(json["agent"], "omp");
    }

    #[test]
    fn unpacks_under_the_package_prefix() {
        let bytes = tarball(&[
            ("package/package.json", r#"{"name":"pi-edgee"}"#),
            ("package/src/index.ts", "export default 1;"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        unpack_package(&bytes, dir.path()).unwrap();
        assert!(dir.path().join("package.json").is_file());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/index.ts")).unwrap(),
            "export default 1;"
        );
    }

    #[test]
    fn rejects_paths_that_escape_the_destination() {
        // `Builder` refuses `..` itself, so write the raw header bytes.
        let mut header = tar::Header::new_gnu();
        let content = b"x";
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        let name = b"package/../evil";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        let mut raw = Vec::new();
        raw.extend_from_slice(header.as_bytes());
        let mut block = content.to_vec();
        block.resize(512, 0);
        raw.extend_from_slice(&block);
        raw.extend_from_slice(&[0u8; 1024]);
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &raw).unwrap();
        let bytes = encoder.finish().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let err = unpack_package(&bytes, dir.path()).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err:#}");
        assert!(!dir.path().parent().unwrap().join("evil").exists());
    }

    #[test]
    fn requires_a_package_json() {
        let bytes = tarball(&[("package/README.md", "hi")]);
        let dir = tempfile::tempdir().unwrap();
        assert!(unpack_package(&bytes, dir.path()).is_err());
    }

    #[test]
    fn verifies_sha512_integrity() {
        let bytes = tarball(&[("package/package.json", "{}")]);
        verify_integrity(&bytes, &integrity_of(&bytes)).unwrap();
        assert!(verify_integrity(&bytes, &integrity_of(b"other")).is_err());
        assert!(verify_integrity(&bytes, "sha1-abc").is_err());
    }

    #[test]
    fn picks_the_sha512_entry_from_a_multi_hash_string() {
        let bytes = b"payload";
        let combined = format!("sha1-zzz {}", integrity_of(bytes));
        verify_integrity(bytes, &combined).unwrap();
    }

    #[test]
    fn cache_layout_is_name_then_version() {
        assert_eq!(
            package_dir(Path::new("/c"), "pi-edgee", "0.3.0"),
            PathBuf::from("/c/pi-edgee/0.3.0")
        );
    }
}
