//! Fetch, reconcile, report — the launch-time entry point.
//!
//! Everything here is best-effort by construction. The established convention is
//! that no launch-time network call may stop the agent starting (`fetch_active_org`
//! returns `Option`, `fetch_model_catalog` returns an empty map), and plugin
//! delivery is no different: a member with a flaky connection gets whatever was
//! materialized last time, not a failed launch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api::{ApiClient, Plugin};
use crate::config::{self, Credentials};

use super::cache::{self, SyncOutcome};
use super::delivery::Target;
use super::materialize::{plan_tree, Plan};
use super::writer;

/// The API can be slow; a launch cannot. `ApiClient` carries a single 30s
/// timeout, which is far too long to make someone wait before their agent
/// starts, so the plugin fetch gets its own much shorter budget.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

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
    // the files derived from them are stale. Start clean instead.
    if root.exists() && !cache::layout_matches(&root) {
        let _ = writer::wipe(&root, &guard);
    }

    let fetched = fetch(token, org_id).await;
    let cached = cache::read(&root, org_id);
    let from_cache = fetched.is_none() && cached.is_some();

    let plugins = match cache::resolve(fetched, cached) {
        SyncOutcome::Reconcile(plugins) => {
            let _ = cache::write(&root, org_id, &plugins, &[]);
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
async fn fetch(token: &str, org_id: &str) -> Option<Vec<Plugin>> {
    let client = ApiClient::new(token).ok()?;
    tokio::time::timeout(FETCH_TIMEOUT, client.list_plugins(org_id))
        .await
        .ok()?
        .ok()
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
