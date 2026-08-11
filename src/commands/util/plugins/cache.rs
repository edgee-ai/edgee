//! The on-disk plugin cache, and the rule for what to do with it.
//!
//! Two jobs. It lets `edgee plugins` show real state without a network round
//! trip, and it lets a launch stay useful when the API is unreachable. The raw
//! API items are cached rather than the materialized tree, so a CLI upgrade that
//! changes the layout re-materializes correctly instead of being stuck with a
//! tree it no longer knows how to produce.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::api::Plugin;

/// Bumped when the materialized layout changes shape. A mismatch wipes every
/// agent tree and rebuilds, which per-plugin change detection cannot do — the
/// plugins themselves may be untouched while the files we derive from them are
/// no longer what this CLI would write.
pub const LAYOUT_VERSION: u32 = 1;

const CACHE_FILE: &str = "index.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheIndex {
    #[serde(default)]
    pub layout_version: u32,
    /// Guards against serving one org's plugins after switching to another.
    #[serde(default)]
    pub org_id: String,
    #[serde(default)]
    pub plugins: Vec<CachedPlugin>,
}

/// A plugin as cached. Deliberately the raw API item, not a digest of it.
pub type CachedPlugin = serde_json::Value;

pub fn cache_path(root: &Path) -> PathBuf {
    root.join(CACHE_FILE)
}

/// Reads the cache, or `None` when absent, unreadable, or from another org.
///
/// Never returns an error: a corrupt cache must behave exactly like a missing
/// one, because the alternative is failing a launch over a file we own.
pub fn read(root: &Path, org_id: &str) -> Option<Vec<Plugin>> {
    let raw = std::fs::read_to_string(cache_path(root)).ok()?;
    let index: CacheIndex = serde_json::from_str(&raw).ok()?;
    if index.org_id != org_id {
        return None;
    }
    Some(
        index
            .plugins
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect(),
    )
}

/// Whether the cached layout still matches what this CLI writes.
pub fn layout_matches(root: &Path) -> bool {
    std::fs::read_to_string(cache_path(root))
        .ok()
        .and_then(|raw| serde_json::from_str::<CacheIndex>(&raw).ok())
        .is_some_and(|index| index.layout_version == LAYOUT_VERSION)
}

pub fn write(
    root: &Path,
    org_id: &str,
    plugins: &[Plugin],
    raw: &[serde_json::Value],
) -> Result<()> {
    let index = CacheIndex {
        layout_version: LAYOUT_VERSION,
        org_id: org_id.to_string(),
        // Prefer the untouched server payload; fall back to a re-serialization
        // when the caller only has parsed values (the install path).
        plugins: if raw.is_empty() && !plugins.is_empty() {
            plugins
                .iter()
                .filter_map(|p| serde_json::to_value(SerializablePlugin::from(p)).ok())
                .collect()
        } else {
            raw.to_vec()
        },
    };
    std::fs::create_dir_all(root)?;
    let path = cache_path(root);
    let tmp = path.with_extension("json.edgee-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&index)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// `api::Plugin` is deserialize-only, matching the rest of the API types. This
/// is the minimum needed to round-trip one back into the cache after an install.
#[derive(Serialize)]
struct SerializablePlugin<'a> {
    id: &'a str,
    name: &'a str,
    display_name: &'a str,
    version: &'a str,
    description: &'a str,
    mode: &'a str,
    targeted: bool,
    active: bool,
    updated_at: &'a str,
}

impl<'a> From<&'a Plugin> for SerializablePlugin<'a> {
    fn from(p: &'a Plugin) -> Self {
        Self {
            id: &p.id,
            name: &p.name,
            display_name: &p.display_name,
            version: &p.version,
            description: &p.description,
            mode: &p.mode,
            targeted: p.targeted,
            active: p.active,
            updated_at: &p.updated_at,
        }
    }
}

/// What a sync should do with what it managed to obtain.
#[derive(Debug, PartialEq)]
pub enum SyncOutcome {
    /// A definitive server answer. Reconcile to it and refresh the cache.
    Reconcile(Vec<Plugin>),
    /// The fetch failed but we have a usable cache. Leave the tree alone and
    /// keep whatever is already materialized.
    UseCache(Vec<Plugin>),
    /// Nothing to act on. The launch proceeds exactly as it would have before
    /// plugins existed.
    DoNothing,
}

/// The whole offline contract, as one pure function.
///
/// The governing rule, borrowed from `ensure_valid_provider_key`: **only a
/// definitive server answer changes local state.** A transient failure keeps
/// what we have — it never deletes. That is why an empty `Some(vec![])` and a
/// `None` are treated so differently: the first means "you have no plugins", the
/// second means "we could not ask".
pub fn resolve(fetched: Option<Vec<Plugin>>, cached: Option<Vec<Plugin>>) -> SyncOutcome {
    match (fetched, cached) {
        // Definitive, including the empty case — that is how an unassignment
        // propagates, so it must reconcile rather than be mistaken for a failure.
        (Some(plugins), _) => SyncOutcome::Reconcile(plugins),
        (None, Some(cached)) => SyncOutcome::UseCache(cached),
        (None, None) => SyncOutcome::DoNothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(name: &str, active: bool) -> Plugin {
        Plugin {
            id: format!("plg_{name}"),
            name: name.to_string(),
            active,
            updated_at: "2026-08-06T12:00:00Z".to_string(),
            ..Default::default()
        }
    }

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn cache_round_trips() {
        let dir = root();
        let plugins = vec![plugin("house", true)];
        write(dir.path(), "org_1", &plugins, &[]).unwrap();

        let back = read(dir.path(), "org_1").expect("cache should be readable");

        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "house");
        assert!(back[0].active);
        assert!(layout_matches(dir.path()));
    }

    /// Switching org must not serve the previous org's plugins.
    #[test]
    fn a_cache_from_another_org_is_ignored() {
        let dir = root();
        write(dir.path(), "org_1", &[plugin("house", true)], &[]).unwrap();

        assert!(read(dir.path(), "org_2").is_none());
        assert!(read(dir.path(), "org_1").is_some());
    }

    /// A file we own being corrupt must never fail a launch.
    #[test]
    fn a_corrupt_cache_reads_as_absent() {
        let dir = root();
        std::fs::write(cache_path(dir.path()), "{ not json").unwrap();

        assert!(read(dir.path(), "org_1").is_none());
        assert!(!layout_matches(dir.path()));
    }

    #[test]
    fn a_missing_cache_reads_as_absent() {
        let dir = root();
        assert!(read(dir.path(), "org_1").is_none());
        assert!(!layout_matches(dir.path()));
    }

    #[test]
    fn a_stale_layout_version_is_detected() {
        let dir = root();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            cache_path(dir.path()),
            serde_json::to_string(&serde_json::json!({
                "layout_version": LAYOUT_VERSION + 1,
                "org_id": "org_1",
                "plugins": []
            }))
            .unwrap(),
        )
        .unwrap();

        // Still readable as data…
        assert!(read(dir.path(), "org_1").is_some());
        // …but the tree derived from it is not what this CLI would write.
        assert!(!layout_matches(dir.path()));
    }

    /// The six rows of the offline contract, with no I/O.
    #[test]
    fn resolve_encodes_the_offline_contract() {
        let fetched = vec![plugin("a", true)];
        let cached = vec![plugin("b", true)];

        // A definitive answer always wins, even when a cache exists.
        assert_eq!(
            resolve(Some(fetched.clone()), Some(cached.clone())),
            SyncOutcome::Reconcile(fetched.clone())
        );
        assert_eq!(
            resolve(Some(fetched.clone()), None),
            SyncOutcome::Reconcile(fetched)
        );

        // Zero active plugins is a definitive answer, not a failure — this is
        // what sweeps the tree when a plugin is unassigned.
        assert_eq!(
            resolve(Some(vec![]), Some(cached.clone())),
            SyncOutcome::Reconcile(vec![])
        );

        // A failed fetch never deletes.
        assert_eq!(
            resolve(None, Some(cached.clone())),
            SyncOutcome::UseCache(cached)
        );
        assert_eq!(resolve(None, None), SyncOutcome::DoNothing);
    }
}
