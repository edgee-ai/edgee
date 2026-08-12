//! The on-disk plugin cache, and the rule for what to do with it.
//!
//! Three jobs. It lets `edgee plugins` show real state without a network round
//! trip, it lets a launch stay useful when the API is unreachable, and it holds
//! the component bodies a launch would otherwise re-download every time. The raw
//! API items are cached rather than the materialized tree, so a CLI upgrade that
//! changes the layout re-materializes correctly instead of being stuck with a
//! tree it no longer knows how to produce.
//!
//! Caching the *bodies* is what makes the offline promise real. An entry that
//! held only names would let a cache-served launch plan an empty tree, and
//! `writer::reconcile` sweeps whatever is not in the plan — so the cache would
//! delete the very files it exists to protect.

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
    pub plugins: Vec<CacheEntry>,
}

/// A cached plugin, plus the revision its component bodies belong to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub plugin: Plugin,
    /// The revision whose bodies are in `plugin`, or `None` when this entry came
    /// from a metadata-only response.
    ///
    /// This is deliberately not inferred from whether the component vectors are
    /// empty: a plugin can legitimately carry nothing, and that must not read as
    /// "bodies missing, fetch again on every launch".
    #[serde(default)]
    pub fetched_revision: Option<u64>,
    /// The `updated_at` those same bodies came with.
    ///
    /// Carried *as well as* the revision because the two do not move together:
    /// components live in their own row server-side, and editing one bumps
    /// `updated_at` while leaving `revision` where it was. Keying freshness on
    /// the revision alone therefore pins a member to whatever bodies were cached
    /// first — a subagent added to an existing plugin never reaches disk.
    #[serde(default)]
    pub fetched_updated_at: Option<String>,
}

impl CacheEntry {
    /// Metadata only, no bodies — what we store for a plugin that is not active
    /// for this member, and therefore never materialized.
    pub fn metadata_only(plugin: Plugin) -> Self {
        Self {
            plugin,
            fetched_revision: None,
            fetched_updated_at: None,
        }
    }

    /// A full plugin as the server just sent it.
    pub fn complete(plugin: Plugin) -> Self {
        let revision = plugin.revision;
        let updated_at = plugin.updated_at.clone();
        Self {
            plugin,
            fetched_revision: Some(revision),
            fetched_updated_at: Some(updated_at),
        }
    }

    /// Whether this entry's bodies can stand in for `meta`.
    ///
    /// Both stamps must agree, and both must be usable. Revision 0 is what a
    /// plugin stored before the server grew the field unmarshals to, and an
    /// empty `updated_at` says the same thing — treating either as a match would
    /// pin the member to whatever bodies happened to be cached first.
    pub fn has_bodies_for(&self, meta: &Plugin) -> bool {
        if meta.revision == 0 || meta.updated_at.is_empty() {
            return false;
        }
        self.fetched_revision == Some(meta.revision)
            && self.fetched_updated_at.as_deref() == Some(meta.updated_at.as_str())
    }
}

pub fn cache_path(root: &Path) -> PathBuf {
    root.join(CACHE_FILE)
}

/// Reads the cache, or `None` when absent, unreadable, or from another org.
///
/// Never returns an error: a corrupt cache must behave exactly like a missing
/// one, because the alternative is failing a launch over a file we own.
pub fn read(root: &Path, org_id: &str) -> Option<Vec<CacheEntry>> {
    let raw = std::fs::read_to_string(cache_path(root)).ok()?;
    let index: CacheIndex = serde_json::from_str(&raw).ok()?;
    if index.org_id != org_id {
        return None;
    }
    Some(index.plugins)
}

/// Whether the cached layout still matches what this CLI writes.
pub fn layout_matches(root: &Path) -> bool {
    std::fs::read_to_string(cache_path(root))
        .ok()
        .and_then(|raw| serde_json::from_str::<CacheIndex>(&raw).ok())
        .is_some_and(|index| index.layout_version == LAYOUT_VERSION)
}

pub fn write(root: &Path, org_id: &str, entries: &[CacheEntry]) -> Result<()> {
    let index = CacheIndex {
        layout_version: LAYOUT_VERSION,
        org_id: org_id.to_string(),
        plugins: entries.to_vec(),
    };
    std::fs::create_dir_all(root)?;
    let path = cache_path(root);
    let tmp = path.with_extension("json.edgee-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&index)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The plugins out of a set of cache entries, in order.
pub fn plugins_of(entries: &[CacheEntry]) -> Vec<Plugin> {
    entries.iter().map(|e| e.plugin.clone()).collect()
}

/// What a fresh metadata list, held against the cache, says still has to be
/// downloaded.
#[derive(Debug, Default, PartialEq)]
pub struct MergePlan {
    /// Usable as they are: metadata from the server, bodies from the cache.
    pub ready: Vec<CacheEntry>,
    /// Active plugins whose bodies the cache cannot supply at this revision.
    pub stale: Vec<Plugin>,
}

/// Splits a metadata list into what the cache already covers and what must be
/// fetched.
///
/// Two rules carry this function.
///
/// **Metadata always comes from the server, never from the cache.** `targeted`
/// and `active` are computed per caller, so adding a member to a squad flips
/// them without touching the plugin — and therefore without moving its revision.
/// An entry that kept its cached flags would sit there permanently out of the
/// tree, waiting for an edit that has no reason to come.
///
/// **Bodies are only worth fetching for active plugins**, the sole thing
/// `plan_tree` materializes. An inactive one keeps whatever bodies it already
/// had: they cost nothing to carry, and if a squad change later makes it active
/// they save the round trip.
pub fn merge(fresh: &[Plugin], cached: Option<&[CacheEntry]>) -> MergePlan {
    let mut plan = MergePlan {
        ready: Vec::with_capacity(fresh.len()),
        stale: Vec::new(),
    };

    for meta in fresh {
        let hit = cached.and_then(|entries| entries.iter().find(|e| e.plugin.id == meta.id));

        let reuse = match hit {
            // Bodies at the right revision, or a plugin we will not materialize
            // anyway — either way there is nothing to ask the server for.
            Some(entry) if entry.has_bodies_for(meta) || !meta.active => Some(entry),
            _ => None,
        };

        match reuse {
            Some(entry) => plan.ready.push(CacheEntry {
                plugin: with_bodies_from(meta, &entry.plugin),
                fetched_revision: entry.fetched_revision,
                fetched_updated_at: entry.fetched_updated_at.clone(),
            }),
            None if meta.active => plan.stale.push(meta.clone()),
            None => plan.ready.push(CacheEntry::metadata_only(meta.clone())),
        }
    }

    plan
}

/// Fresh metadata wearing the cache's component bodies.
fn with_bodies_from(fresh: &Plugin, cached: &Plugin) -> Plugin {
    Plugin {
        skills: cached.skills.clone(),
        subagents: cached.subagents.clone(),
        hooks: cached.hooks.clone(),
        mcp_servers: cached.mcp_servers.clone(),
        ..fresh.clone()
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
            revision: 1,
            updated_at: "2026-08-06T12:00:00Z".to_string(),
            ..Default::default()
        }
    }

    /// A plugin carrying one skill, so a round trip can prove the body survived.
    fn with_skill(mut p: Plugin, body: &str) -> Plugin {
        p.skills = vec![crate::api::PluginSkill {
            name: "commit-style".to_string(),
            description: "How this team writes commit messages.".to_string(),
            body: body.to_string(),
            ..Default::default()
        }];
        p.component_counts = crate::api::PluginComponentCounts {
            skill: 1,
            ..Default::default()
        };
        p
    }

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// The bodies are the point of the cache: an entry that kept only names
    /// would let an offline launch plan an empty tree, and reconcile would then
    /// delete every delivered file.
    #[test]
    fn cache_round_trips_component_bodies() {
        let dir = root();
        let entries = vec![CacheEntry::complete(with_skill(
            plugin("house", true),
            "Use the imperative mood.",
        ))];
        write(dir.path(), "org_1", &entries).unwrap();

        let back = read(dir.path(), "org_1").expect("cache should be readable");

        assert_eq!(back.len(), 1);
        assert_eq!(back[0].plugin.name, "house");
        assert!(back[0].plugin.active);
        assert_eq!(back[0].plugin.skills.len(), 1);
        assert_eq!(back[0].plugin.skills[0].body, "Use the imperative mood.");
        assert_eq!(back[0].fetched_revision, Some(1));
        assert_eq!(
            back[0].fetched_updated_at.as_deref(),
            Some("2026-08-06T12:00:00Z")
        );
        assert!(layout_matches(dir.path()));
    }

    /// Switching org must not serve the previous org's plugins.
    #[test]
    fn a_cache_from_another_org_is_ignored() {
        let dir = root();
        write(
            dir.path(),
            "org_1",
            &[CacheEntry::complete(plugin("house", true))],
        )
        .unwrap();

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

    /// Fresh metadata plus a matching cache means no request: the bodies we hold
    /// are the bodies the server has.
    #[test]
    fn merge_reuses_cached_bodies_at_the_same_revision() {
        let cached = vec![CacheEntry::complete(with_skill(
            plugin("house", true),
            "Use the imperative mood.",
        ))];
        // Same revision, but a metadata-only payload — no bodies on the wire.
        let fresh = vec![plugin("house", true)];

        let plan = merge(&fresh, Some(&cached));

        assert!(plan.stale.is_empty(), "nothing should need fetching");
        assert_eq!(plan.ready.len(), 1);
        assert_eq!(plan.ready[0].plugin.skills.len(), 1);
        assert_eq!(
            plan.ready[0].plugin.skills[0].body,
            "Use the imperative mood."
        );
    }

    /// An edit moves the revision, and the cached bodies stop counting.
    #[test]
    fn merge_refetches_when_the_revision_moved() {
        let cached = vec![CacheEntry::complete(with_skill(
            plugin("house", true),
            "Use the imperative mood.",
        ))];
        let mut fresh = plugin("house", true);
        fresh.revision = 2;

        let plan = merge(&[fresh], Some(&cached));

        assert!(plan.ready.is_empty());
        assert_eq!(plan.stale.len(), 1);
        assert_eq!(plan.stale[0].id, "plg_house");
    }

    /// Revision 0 is what a plugin stored before the server grew the field
    /// unmarshals to. Matching on it would pin the member to stale bodies.
    #[test]
    fn merge_never_trusts_revision_zero() {
        let mut stored = with_skill(plugin("house", true), "Old.");
        stored.revision = 0;
        let cached = vec![CacheEntry {
            plugin: stored,
            fetched_revision: Some(0),
            fetched_updated_at: Some("2026-08-06T12:00:00Z".to_string()),
        }];
        let mut fresh = plugin("house", true);
        fresh.revision = 0;

        let plan = merge(&[fresh], Some(&cached));

        assert!(plan.ready.is_empty());
        assert_eq!(plan.stale.len(), 1);
    }

    /// The bug this pairing exists for. Components live in their own row
    /// server-side, so adding a subagent to an existing plugin bumps
    /// `updated_at` and leaves `revision` alone. Keyed on the revision alone the
    /// cache calls its bodies current forever, and the new subagent never
    /// reaches disk — the plugin looks delivered while missing the very thing
    /// that was just added to it.
    #[test]
    fn merge_refetches_when_only_updated_at_moved() {
        let cached = vec![CacheEntry::complete(with_skill(
            plugin("house", true),
            "Use the imperative mood.",
        ))];
        let mut fresh = plugin("house", true);
        fresh.updated_at = "2026-08-12T17:00:57Z".to_string();
        assert_eq!(
            fresh.revision, cached[0].plugin.revision,
            "revision stands still"
        );

        let plan = merge(&[fresh], Some(&cached));

        assert!(plan.ready.is_empty());
        assert_eq!(plan.stale.len(), 1, "an edited plugin must be re-fetched");
    }

    /// An `updated_at` the server did not send is as unusable as revision 0.
    #[test]
    fn merge_never_trusts_an_empty_updated_at() {
        let mut stored = with_skill(plugin("house", true), "Old.");
        stored.updated_at = String::new();
        let cached = vec![CacheEntry::complete(stored)];
        let mut fresh = plugin("house", true);
        fresh.updated_at = String::new();

        let plan = merge(&[fresh], Some(&cached));

        assert!(plan.ready.is_empty());
        assert_eq!(plan.stale.len(), 1);
    }

    /// The flags are computed per caller, so a squad change flips them without
    /// touching the plugin — and therefore without moving its revision. Reading
    /// them from the cache would strand the plugin outside the tree.
    #[test]
    fn merge_takes_the_flags_from_the_server_not_the_cache() {
        let cached = vec![CacheEntry::complete(with_skill(
            plugin("house", false),
            "Use the imperative mood.",
        ))];
        // Same revision: only the caller's squad membership changed.
        let fresh = vec![plugin("house", true)];

        let plan = merge(&fresh, Some(&cached));

        assert!(plan.stale.is_empty(), "the bodies were already cached");
        assert_eq!(plan.ready.len(), 1);
        assert!(
            plan.ready[0].plugin.active,
            "expected the fresh flag to win over the cached one"
        );
        assert_eq!(plan.ready[0].plugin.skills.len(), 1);
    }

    /// Only active plugins get materialized, so only they are worth a request.
    #[test]
    fn merge_does_not_fetch_bodies_for_inactive_plugins() {
        let fresh = vec![plugin("offered", false)];

        let plan = merge(&fresh, None);

        assert!(plan.stale.is_empty());
        assert_eq!(plan.ready.len(), 1);
        assert_eq!(
            plan.ready[0].fetched_revision, None,
            "an entry with no bodies must say so, or it will be trusted later"
        );
    }

    /// An empty cache is not a special case, just a total miss.
    #[test]
    fn merge_without_a_cache_fetches_every_active_plugin() {
        let fresh = vec![plugin("a", true), plugin("b", true), plugin("c", false)];

        let plan = merge(&fresh, None);

        assert_eq!(plan.stale.len(), 2);
        assert_eq!(plan.ready.len(), 1);
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
