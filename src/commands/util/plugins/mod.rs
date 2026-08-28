//! Org plugin delivery.
//!
//! An admin authors a plugin in the console — skills, subagents, hooks, MCP
//! servers — and targets it at the org, at squads, or at named members. This
//! module turns what the API says is active for *this* user into files on disk,
//! and hands each coding agent a way to read them.
//!
//! The rule the whole design turns on: **Edgee materializes into a directory it
//! owns and points the agent at it.** It never writes into a user-owned config
//! file, and never into a git working tree. A component kind that cannot be
//! redirected for a given agent is reported as undelivered rather than worked
//! around — see [`delivery`].

pub mod cache;
pub mod config;
pub mod delivery;
pub mod materialize;
pub mod mirror;
pub mod report;
pub mod sync;
pub mod writer;

pub use delivery::{Kind, Target};

/// The Codex config root to mirror: whatever `CODEX_HOME` already names, else
/// the default `~/.codex`. Honouring an existing override matters — a user who
/// has relocated their Codex config should have *that* one mirrored.
pub fn codex_home_source() -> std::path::PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| crate::config::edgee_home().and_then(|h| h.parent().map(|p| p.join(".codex"))))
        .unwrap_or_else(|| std::path::PathBuf::from(".codex"))
}

/// Where the mirrored Codex config root is built. Lives beside the materialized
/// trees so it is covered by the same reconcile guard.
pub fn codex_home_mirror() -> Option<std::path::PathBuf> {
    Some(crate::config::plugins_dir()?.join("codex-home"))
}
pub use report::report_launch;
pub use sync::sync_for_target;
