//! Fetch, reconcile, report — the launch-time entry point.
//!
//! Everything here is best-effort by construction. The established convention is
//! that no launch-time network call may stop the agent starting (`fetch_active_org`
//! returns `Option`, `fetch_model_catalog` returns an empty map), and plugin
//! delivery is no different: a member with a flaky connection gets whatever was
//! materialized last time, not a failed launch.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::api::{ApiClient, Plugin};
use crate::config::{self, Credentials};

use super::cache::{self, CacheEntry, SyncOutcome};
use super::delivery::Target;
use super::materialize::{plan_tree, Plan};
use super::writer;

/// The API can be slow; a launch cannot. `ApiClient` carries a single 30s
/// timeout, which is far too long to make someone wait before their agent
/// starts, so the plugin fetch gets its own much shorter budget.
///
/// Two budgets, not one, because the two phases fail differently. The metadata
/// list is a single small request. The bodies are a fan-out whose size depends
/// on how much changed — so it gets its own deadline covering the whole batch,
/// not one per request, and a slow server cannot multiply the wait by the number
/// of edited plugins.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
const BODY_TIMEOUT: Duration = Duration::from_secs(5);

/// What the launch path needs to know once delivery is done.
#[derive(Debug, Default)]
pub struct SyncReport {
    /// Absolute directories to hand the agent, one per delivered plugin.
    pub plugin_dirs: Vec<PathBuf>,
    pub plugin_count: usize,
    pub skills: usize,
    pub subagents: usize,
    pub hooks: usize,
    pub mcp_servers: usize,
    /// Kinds that had components but this agent cannot take.
    pub undelivered: Vec<super::Kind>,
    /// The plugins in force for this user, for targets that build config
    /// fragments (Crush, OpenCode) rather than taking a bundle directory.
    pub plugins: Vec<Plugin>,
    /// Root holding this target's namespaced skill directories, when it
    /// materialized any. `None` when the target got no skills, so a launch never
    /// points an agent at an empty or absent directory.
    pub skills_root: Option<PathBuf>,
    /// True when the fetch failed and materialization came from cache.
    pub from_cache: bool,
}

impl SyncReport {
    pub fn is_empty(&self) -> bool {
        self.plugin_count == 0
    }

    /// `3 skills · 1 subagent · 2 MCP servers`, omitting what is absent.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for (n, one, many) in [
            (self.skills, "skill", "skills"),
            (self.subagents, "subagent", "subagents"),
            (self.hooks, "hook", "hooks"),
            (self.mcp_servers, "MCP server", "MCP servers"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        }
        parts.join(" · ")
    }
}

/// Materializes the org's active plugins for one agent and returns what to pass it.
///
/// Never returns `Err` to the launch path — every failure degrades to delivering
/// less, never to blocking the agent.
pub async fn sync_for_target(creds: &Credentials, target: Target) -> SyncReport {
    if config::plugins_disabled_env_override().unwrap_or(false) {
        return SyncReport::default();
    }

    let (Some(token), Some(org_id)) = (creds.user_token.as_ref(), creds.org_id.as_ref()) else {
        return SyncReport::default();
    };
    let (Some(root), Some(guard)) = (config::plugins_dir(), config::edgee_home()) else {
        return SyncReport::default();
    };

    // A layout this CLI no longer writes cannot be reconciled into shape by
    // per-plugin change detection, because the plugins may be untouched while
    // the files derived from them are stale. The tree has to be rebuilt from
    // scratch — but only once we hold an answer to rebuild it *from*. Wiping
    // first would mean a failed fetch leaves the member with nothing at all,
    // which is exactly the deletion-on-transient-failure this module forbids.
    let stale_layout = root.exists() && !cache::layout_matches(&root);

    // A cache written under a layout we no longer produce is not a cache we can
    // serve: `resolve` would keep the existing tree, and the existing tree is
    // the thing that is wrong.
    let cached = if stale_layout {
        None
    } else {
        cache::read(&root, org_id)
    };
    let fetched = fetch(token, org_id, cached.as_deref()).await;
    let from_cache = fetched.is_none() && cached.is_some();

    let plugins = match cache::resolve(
        fetched.as_deref().map(cache::plugins_of),
        cached.as_deref().map(cache::plugins_of),
    ) {
        SyncOutcome::Reconcile(plugins) => {
            // Definitive answer in hand: now the old-layout tree can go.
            if stale_layout {
                let _ = writer::wipe(&root, &guard);
            }
            // `fetched` is what produced this arm, so it is always Some here.
            if let Some(entries) = &fetched {
                let _ = cache::write(&root, org_id, entries);
            }
            plugins
        }
        SyncOutcome::UseCache(plugins) => plugins,
        SyncOutcome::DoNothing => return SyncReport::default(),
    };

    let plan = plan_tree(&plugins, target);
    let target_root = root.join(target.dir());
    if writer::reconcile(&target_root, &plan, &guard).is_err() {
        // A tree we could not write is a tree we must not advertise.
        return SyncReport::default();
    }

    let mut report = report_for(&plan, &target_root);
    report.from_cache = from_cache;
    report.plugins = plugins;
    report
}

/// `None` means "we could not ask" — distinct from an empty list, which means
/// "you have no plugins" and does sweep the tree.
///
/// Two phases. The metadata list says what exists and at which revision; the
/// bodies are then fetched only for the active plugins the cache cannot cover.
/// A launch that changed nothing since the last one issues exactly one request
/// and downloads no component bodies at all.
///
/// A body we cannot obtain collapses the whole thing back to `None`. Returning a
/// partial set would be worse than returning nothing: `writer::reconcile` sweeps
/// every file that is not in the plan, so a plugin missing from a "successful"
/// answer is a plugin deleted from disk. Only a complete answer changes local
/// state — the same rule `cache::resolve` encodes for the offline case.
async fn fetch(
    token: &str,
    org_id: &str,
    cached: Option<&[CacheEntry]>,
) -> Option<Vec<CacheEntry>> {
    let client = Arc::new(ApiClient::new(token).ok()?);

    let metadata = tokio::time::timeout(LIST_TIMEOUT, client.list_plugins_metadata(org_id))
        .await
        .ok()?
        .ok()?;

    let mut plan = cache::merge(&metadata, cached);
    if plan.stale.is_empty() {
        return Some(plan.ready);
    }

    let fetched = tokio::time::timeout(BODY_TIMEOUT, fetch_bodies(client, org_id, &plan.stale))
        .await
        .ok()??;

    plan.ready.extend(fetched);
    Some(plan.ready)
}

/// Downloads the component bodies for `stale`, concurrently. `None` if any of
/// them fails.
///
/// A `JoinSet` rather than loose handles because dropping it aborts whatever is
/// still in flight — so when the deadline above fires, or one request fails, the
/// rest stop with it instead of running on behind a launch that already moved.
async fn fetch_bodies(
    client: Arc<ApiClient>,
    org_id: &str,
    stale: &[Plugin],
) -> Option<Vec<CacheEntry>> {
    let mut requests = tokio::task::JoinSet::new();
    for meta in stale {
        let client = Arc::clone(&client);
        let org_id = org_id.to_string();
        let plugin_id = meta.id.clone();
        requests.spawn(async move { client.get_plugin(&org_id, &plugin_id).await });
    }

    let mut entries = Vec::with_capacity(stale.len());
    while let Some(joined) = requests.join_next().await {
        entries.push(CacheEntry::complete(joined.ok()?.ok()?));
    }
    Some(entries)
}

fn report_for(plan: &Plan, root: &Path) -> SyncReport {
    // Only advertise the skills root when something is actually in it.
    let skills_root = plan
        .files
        .iter()
        .any(|f| f.path.starts_with("skills"))
        .then(|| root.join("skills"));

    let mut report = SyncReport {
        skills_root,
        plugin_dirs: plan.plugin_dirs.iter().map(|d| root.join(d)).collect(),
        plugin_count: plan.plugin_dirs.len(),
        undelivered: plan.undelivered_kinds(),
        ..Default::default()
    };

    // Count what was actually delivered, not what the plugin declared — a kind
    // this agent cannot take must not be advertised as installed.
    for outcome in plan.report.iter().filter(|o| o.delivered) {
        match outcome.kind {
            super::Kind::Skills => report.skills += outcome.count,
            super::Kind::Subagents => report.subagents += outcome.count,
            super::Kind::Hooks => report.hooks += outcome.count,
            super::Kind::McpServers => report.mcp_servers += outcome.count,
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(skills: usize, subagents: usize, mcp: usize) -> SyncReport {
        SyncReport {
            plugin_count: 1,
            skills,
            subagents,
            mcp_servers: mcp,
            ..Default::default()
        }
    }

    #[test]
    fn summary_omits_absent_kinds_and_singularizes() {
        assert_eq!(report(3, 1, 0).summary(), "3 skills · 1 subagent");
        assert_eq!(report(1, 0, 2).summary(), "1 skill · 2 MCP servers");
        assert_eq!(report(0, 0, 0).summary(), "");
    }

    #[test]
    fn an_empty_report_is_empty() {
        assert!(SyncReport::default().is_empty());
        assert!(!report(1, 0, 0).is_empty());
    }
}
