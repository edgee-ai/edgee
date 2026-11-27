//! Prompt-free `edgee settings <agent>`: print the key's compression state, or
//! apply `--set`s, for scripts, CI and coding agents.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use console::style;
use serde::Serialize;

use super::{SetArg, Technique};
use crate::api::{ApiClient, Compression, KeyCompression, KeyListItem, KeySettings};
use crate::commands::auth::login;
use crate::commands::util;
use crate::config::Credentials;

#[derive(Debug, PartialEq, Eq, Serialize)]
struct TechniqueState {
    /// What the gateway applies: the key's value resolved against org/squad scope.
    enabled: bool,
    /// The key stores no value of its own for this technique (it inherits).
    inherited: bool,
}

/// Machine-readable shape of `edgee settings <agent> --json`.
#[derive(Debug, Serialize)]
struct SettingsJson {
    agent: String,
    changed: bool,
    techniques: BTreeMap<&'static str, TechniqueState>,
}

/// What a show or apply ended with.
struct Outcome {
    /// The key's own values after the command.
    raw: KeyCompression,
    /// The values the gateway applies after the command.
    resolved: Compression,
    changed: bool,
}

pub async fn run(provider: &str, sets: &[SetArg], json: bool) -> Result<()> {
    let creds = crate::config::read()?;
    let (client, org_id) = client_and_org(&creds)?;

    let outcome = if sets.is_empty() {
        show(&client, &creds, &org_id, provider).await?
    } else {
        apply(&client, &org_id, provider, sets).await?
    };

    for set in sets {
        if resolved_value(set.technique, &outcome.resolved) != set.enabled {
            eprintln!(
                "warning: {} stays {} for {}: an organization or squad setting overrides this key.",
                set.technique.name(),
                on_off(!set.enabled),
                login::agent_label(provider),
            );
        }
    }

    let techniques = states(&outcome.raw, &outcome.resolved);
    if json {
        return util::emit_json(&SettingsJson {
            agent: provider.to_string(),
            changed: outcome.changed,
            techniques,
        });
    }
    print_human(
        login::agent_label(provider),
        !sets.is_empty(),
        outcome.changed,
        &techniques,
    );
    Ok(())
}

/// Like the wizard's credential checks, but without `ensure_org_selected`: that
/// helper prompts and prints to stdout, which would corrupt `--json` output.
fn client_and_org(creds: &Credentials) -> Result<(ApiClient, String)> {
    let token = creds
        .user_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Not authenticated. Run `edgee auth login` first."))?;
    let org_id = creds
        .org_id
        .as_deref()
        .filter(|o| !o.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No organization selected. Run `edgee auth login --org <id|slug>` first."
            )
        })?;
    Ok((ApiClient::new(token)?, org_id.to_string()))
}

/// Read-only: never creates a key, so inspecting settings has no side effects.
async fn show(
    client: &ApiClient,
    creds: &Credentials,
    org_id: &str,
    provider: &str,
) -> Result<Outcome> {
    let no_key = || {
        anyhow::anyhow!(
            "No {} key found. Launch the agent through Edgee once, or create the key with \
             `edgee settings {provider} --set <technique>=on`.",
            login::agent_label(provider)
        )
    };

    let keys = client.list_keys(org_id).await?;
    let cached_id = creds
        .provider(provider)
        .and_then(|p| p.api_key_id.as_deref());
    let assistant = login::coding_assistant_name(provider)?;
    let key = find_key(&keys, cached_id, assistant, creds.user_id.as_deref()).ok_or_else(no_key)?;

    let resolved = client
        .get_key_by_id(org_id, &key.id)
        .await?
        .ok_or_else(no_key)?
        .compression
        .unwrap_or_default();

    Ok(Outcome {
        raw: key.compression.unwrap_or_default(),
        resolved,
        changed: false,
    })
}

/// Creating the key when missing is fine here, a write was asked for anyway.
async fn apply(
    client: &ApiClient,
    org_id: &str,
    provider: &str,
    sets: &[SetArg],
) -> Result<Outcome> {
    let key = login::fetch_provider_key(provider).await?;

    // The by-id endpoint returns resolved values; the update replaces the key's
    // own values, so the raw ones (nulls included) must come from the list.
    let keys = client.list_keys(org_id).await?;
    let raw = keys
        .iter()
        .find(|k| k.id == key.id)
        .map(|k| k.compression.unwrap_or_default())
        .context("The key was not in the key list. Re-run the command, or run `edgee auth login` if it keeps failing.")?;

    let (raw, changed) = match plan_update(raw, sets) {
        Some(next) => {
            client
                .update_key_settings(org_id, &key.id, &KeySettings { compression: next })
                .await
                .context("Failed to update key settings")?;
            (next, true)
        }
        None => (raw, false),
    };

    let resolved = client
        .get_key_by_id(org_id, &key.id)
        .await?
        .context("The key disappeared after the update. Re-run the command.")?
        .compression
        .unwrap_or_default();

    Ok(Outcome {
        raw,
        resolved,
        changed,
    })
}

/// The key's own values after applying `sets`, or `None` when nothing differs.
/// Unnamed techniques keep their current value, null (inherit) included.
fn plan_update(raw: KeyCompression, sets: &[SetArg]) -> Option<KeyCompression> {
    let mut next = raw;
    for set in sets {
        set.technique.set(&mut next, set.enabled);
    }
    (next != raw).then_some(next)
}

/// The key to act on: the one cached in credentials, else the caller's live key
/// for this agent. The fallback needs `user_id`, because admins see every key.
fn find_key<'a>(
    keys: &'a [KeyListItem],
    cached_id: Option<&str>,
    assistant: &str,
    user_id: Option<&str>,
) -> Option<&'a KeyListItem> {
    if let Some(id) = cached_id {
        if let Some(key) = keys.iter().find(|k| k.id == id) {
            return Some(key);
        }
    }
    let user_id = user_id?;
    keys.iter().find(|k| {
        k.coding_assistant.as_deref() == Some(assistant)
            && k.user_id.as_deref() == Some(user_id)
            && k.active
            && !k.expires_at.is_some_and(login::key_is_expired)
    })
}

fn states(
    raw: &KeyCompression,
    resolved: &Compression,
) -> BTreeMap<&'static str, TechniqueState> {
    Technique::ALL
        .into_iter()
        .map(|t| {
            (
                t.name(),
                TechniqueState {
                    enabled: resolved_value(t, resolved),
                    inherited: t.get(raw).is_none(),
                },
            )
        })
        .collect()
}

fn resolved_value(t: Technique, resolved: &Compression) -> bool {
    match t {
        Technique::ToolResultTrimming => resolved.tool_result_trimming,
        Technique::ToolSurfaceReduction => resolved.tool_surface_reduction,
        Technique::OutputBrevity => resolved.output_brevity,
    }
}

fn on_off(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

fn print_human(
    label: &str,
    applied: bool,
    changed: bool,
    techniques: &BTreeMap<&'static str, TechniqueState>,
) {
    let header = match (applied, changed) {
        (false, _) => format!("Settings for {label}"),
        (true, true) => format!("Settings updated for {label}"),
        (true, false) => format!("No changes for {label}, settings already match"),
    };
    println!();
    println!("  {}", style(header).bold());
    for (name, state) in techniques {
        let value = if state.enabled {
            style("on").green()
        } else {
            style("off").dim()
        };
        let inherited = if state.inherited {
            style(" (inherited)").dim().to_string()
        } else {
            String::new()
        };
        println!("      {name:<24} {value}{inherited}");
    }
    println!();
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::*;

    fn set(technique: Technique, enabled: bool) -> SetArg {
        SetArg { technique, enabled }
    }

    fn key(id: &str, user: &str, assistant: &str) -> KeyListItem {
        KeyListItem {
            id: id.into(),
            user_id: Some(user.into()),
            coding_assistant: Some(assistant.into()),
            active: true,
            expires_at: None,
            compression: None,
        }
    }

    #[test]
    fn plan_update_is_none_when_values_already_match() {
        let raw = KeyCompression {
            output_brevity: Some(true),
            ..Default::default()
        };
        assert_eq!(plan_update(raw, &[set(Technique::OutputBrevity, true)]), None);
    }

    #[test]
    fn plan_update_keeps_unnamed_nulls_and_values() {
        let raw = KeyCompression {
            tool_result_trimming: None,
            tool_surface_reduction: Some(true),
            output_brevity: None,
        };
        let next = plan_update(raw, &[set(Technique::OutputBrevity, true)]).unwrap();
        assert_eq!(next.tool_result_trimming, None);
        assert_eq!(next.tool_surface_reduction, Some(true));
        assert_eq!(next.output_brevity, Some(true));
    }

    #[test]
    fn pinning_an_inherited_value_counts_as_a_change() {
        let next = plan_update(
            KeyCompression::default(),
            &[set(Technique::OutputBrevity, false)],
        );
        assert_eq!(
            next.map(|c| c.output_brevity),
            Some(Some(false)),
            "null -> explicit false must be written"
        );
    }

    #[test]
    fn states_report_resolved_value_and_inheritance_separately() {
        let raw = KeyCompression {
            tool_result_trimming: None,
            tool_surface_reduction: Some(false),
            output_brevity: Some(true),
        };
        // The org forces tool-surface-reduction on over the key's own `false`.
        let resolved = Compression {
            tool_result_trimming: false,
            tool_surface_reduction: true,
            output_brevity: true,
        };
        let s = states(&raw, &resolved);
        assert_eq!(
            s["tool-result-trimming"],
            TechniqueState {
                enabled: false,
                inherited: true
            }
        );
        assert_eq!(
            s["tool-surface-reduction"],
            TechniqueState {
                enabled: true,
                inherited: false
            }
        );
        assert_eq!(
            s["output-brevity"],
            TechniqueState {
                enabled: true,
                inherited: false
            }
        );
    }

    #[test]
    fn find_key_prefers_the_cached_id() {
        let keys = [key("a", "u1", "claude_code"), key("b", "u1", "claude_code")];
        assert_eq!(find_key(&keys, Some("b"), "claude_code", Some("u1")).unwrap().id, "b");
    }

    #[test]
    fn find_key_falls_back_to_the_callers_live_key_for_the_agent() {
        let mut expired = key("old", "u1", "claude_code");
        expired.expires_at = Some(OffsetDateTime::UNIX_EPOCH);
        let mut inactive = key("off", "u1", "claude_code");
        inactive.active = false;
        let keys = [
            expired,
            inactive,
            key("other-user", "u2", "claude_code"),
            key("other-agent", "u1", "codex"),
            key("good", "u1", "claude_code"),
        ];
        assert_eq!(
            find_key(&keys, Some("stale"), "claude_code", Some("u1")).unwrap().id,
            "good"
        );
    }

    #[test]
    fn find_key_without_a_user_id_never_guesses_by_agent() {
        let keys = [key("a", "u1", "claude_code")];
        assert!(find_key(&keys, None, "claude_code", None).is_none());
    }

    #[test]
    fn json_shape_is_stable() {
        let out = SettingsJson {
            agent: "claude".into(),
            changed: true,
            techniques: states(&KeyCompression::default(), &Compression::default()),
        };
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["agent"], "claude");
        assert_eq!(v["changed"], true);
        assert_eq!(
            v["techniques"]["output-brevity"],
            serde_json::json!({"enabled": false, "inherited": true})
        );
        for name in ["tool-result-trimming", "tool-surface-reduction"] {
            assert!(v["techniques"].get(name).is_some(), "missing `{name}`");
        }
    }
}
