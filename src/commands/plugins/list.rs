//! `edgee plugins list` — what the org has assigned, and what each agent gets.

use anyhow::Result;
use console::style;

use crate::api::{ApiClient, Plugin};
use crate::commands::util::plugins::delivery::{delivery, Delivery, Kind, Target};

use super::org_context;

#[derive(Debug, Default, clap::Parser)]
pub struct Options {
    /// Show every component and the reason anything is not delivered
    #[arg(long)]
    verbose: bool,
}

/// One rendered line. Split from printing so the ordering and labelling can be
/// asserted without a terminal.
#[derive(Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub title: String,
    pub version: String,
    pub state: State,
    pub summary: String,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum State {
    /// In force and not yours to remove — org policy.
    Enforced,
    /// In force because you opted in. You can remove it again.
    Installed,
    /// Offered to you, not installed.
    Available,
    /// Visible only because you are an admin — it targets someone else.
    NotAssigned,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            State::Enforced => "enforced",
            State::Installed => "installed",
            State::Available => "available",
            State::NotAssigned => "not assigned",
        }
    }
}

/// Sorts active first, then what you could install, then the admin-only view.
/// Within a group, by name, so the listing is stable between runs.
pub fn rows(plugins: &[Plugin]) -> Vec<Row> {
    let mut rows: Vec<Row> = plugins
        .iter()
        .map(|p| Row {
            name: p.name.clone(),
            title: p.title().to_string(),
            version: p.version.clone(),
            state: state_of(p),
            summary: summarize(p),
        })
        .collect();
    rows.sort_by(|a, b| {
        let rank = |s: State| match s {
            State::Enforced => 0,
            State::Installed => 1,
            State::Available => 2,
            State::NotAssigned => 3,
        };
        rank(a.state)
            .cmp(&rank(b.state))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

fn state_of(plugin: &Plugin) -> State {
    if plugin.active {
        // Worth distinguishing: one of these the member can undo, the other is
        // org policy and `edgee plugins remove` will refuse it.
        if plugin.is_enforced() {
            State::Enforced
        } else {
            State::Installed
        }
    } else if plugin.targeted {
        State::Available
    } else {
        // Admins receive the whole org catalogue, so an untargeted plugin is not
        // an error — it just is not theirs.
        State::NotAssigned
    }
}

fn summarize(plugin: &Plugin) -> String {
    let mut parts = Vec::new();
    for (n, one, many) in [
        (plugin.skills.len(), "skill", "skills"),
        (plugin.subagents.len(), "subagent", "subagents"),
        (plugin.hooks.len(), "hook", "hooks"),
        (plugin.mcp_servers.len(), "MCP server", "MCP servers"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {}", if n == 1 { one } else { many }));
        }
    }
    if parts.is_empty() {
        "no components".to_string()
    } else {
        parts.join(" · ")
    }
}

pub async fn run(opts: Options) -> Result<()> {
    let (token, org_id) = org_context().await?;
    let plugins = ApiClient::new(&token)?.list_plugins(&org_id).await?;
    let rows = rows(&plugins);

    println!();
    if rows.is_empty() {
        println!("  {}", style("No plugins have been assigned to you.").dim());
        println!(
            "  {}",
            style("An organization admin can create them at www.edgee.ai.").dim()
        );
        println!();
        return Ok(());
    }

    let mut heading_shown = false;
    for row in &rows {
        // The admin-only tail gets its own heading, so an admin is never misled
        // into thinking an untargeted plugin is on their machine.
        if row.state == State::NotAssigned && !heading_shown {
            heading_shown = true;
            println!();
            println!("  {}", style("Not assigned to you (admin view)").dim());
        }

        let (glyph, label) = match row.state {
            State::Enforced | State::Installed => (
                style("✓").green().bold(),
                style(row.state.label()).green(),
            ),
            State::Available => (style("○").dim(), style(row.state.label()).dim()),
            State::NotAssigned => (style("·").dim(), style(row.state.label()).dim()),
        };
        println!(
            "  {glyph} {}  {}  {}  {}",
            style(&row.title).bold(),
            style(&row.version).dim(),
            label,
            style(&row.summary).dim()
        );

        match row.state {
            State::Available => println!(
                "    {}",
                style(format!("edgee plugins install {}", row.name)).dim()
            ),
            State::Installed => println!(
                "    {}",
                style(format!("edgee plugins remove {}", row.name)).dim()
            ),
            _ => {}
        }
    }

    println!();
    print_delivery(&plugins, opts.verbose);
    println!();
    Ok(())
}

/// What each agent actually receives. This is the honest part: a plugin can be
/// assigned to you and still not reach a given assistant, and saying so here is
/// better than the component silently never appearing.
fn print_delivery(plugins: &[Plugin], verbose: bool) {
    let active: Vec<&Plugin> = plugins.iter().filter(|p| p.active).collect();
    if active.is_empty() {
        return;
    }

    let used: Vec<Kind> = Kind::ALL
        .into_iter()
        .filter(|kind| {
            active.iter().any(|p| match kind {
                Kind::Skills => !p.skills.is_empty(),
                Kind::Subagents => !p.subagents.is_empty(),
                Kind::Hooks => !p.hooks.is_empty(),
                Kind::McpServers => !p.mcp_servers.is_empty(),
            })
        })
        .collect();
    if used.is_empty() {
        return;
    }

    println!("  {}", style("Delivery").dim());
    for target in Target::ALL {
        let delivered: Vec<&str> = used
            .iter()
            .filter(|k| delivery(target, **k).is_delivered())
            .map(|k| k.label())
            .collect();
        let missing: Vec<&Kind> = used
            .iter()
            .filter(|k| !delivery(target, **k).is_delivered())
            .collect();

        let summary = if delivered.is_empty() {
            style("nothing yet".to_string()).dim()
        } else if missing.is_empty() {
            style(delivered.join(", ")).green()
        } else {
            style(delivered.join(", ")).dim()
        };
        println!("    {:<10} {summary}", target.label());

        if verbose {
            for kind in missing {
                if let Delivery::Unsupported { reason } = delivery(target, *kind) {
                    println!("      {}", style(format!("{}: {reason}", kind.label())).dim());
                }
            }
        }
    }

    if !verbose {
        println!(
            "    {}",
            style("Run `edgee plugins list --verbose` for what each assistant cannot take.").dim()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(name: &str, targeted: bool, active: bool) -> Plugin {
        Plugin {
            id: format!("plg_{name}"),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            targeted,
            active,
            ..Default::default()
        }
    }

    #[test]
    fn rows_sort_in_force_first_then_available_then_admin_view() {
        let rows = rows(&[
            plugin("zeta-not-mine", false, false),
            plugin("beta-offered", true, false),
            plugin("alpha-active", true, true),
        ]);

        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha-active", "beta-offered", "zeta-not-mine"]
        );
        assert_eq!(rows[0].state, State::Installed);
        assert_eq!(rows[1].state, State::Available);
        assert_eq!(rows[2].state, State::NotAssigned);
    }

    #[test]
    fn rows_are_ordered_by_name_within_a_group() {
        let rows = rows(&[plugin("b", true, true), plugin("a", true, true)]);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    /// Both are in force, but only one is the member's to undo — so the listing
    /// distinguishes them rather than calling both "active".
    #[test]
    fn enforced_and_installed_are_distinguished() {
        let mut enforced = plugin("e", true, true);
        enforced.mode = "enforced".into();
        let mut installed = plugin("i", true, true);
        installed.mode = "optional".into();

        let rows = rows(&[installed, enforced]);

        assert_eq!(rows[0].state, State::Enforced);
        assert_eq!(rows[1].state, State::Installed);
    }

    #[test]
    fn summary_counts_components_and_singularizes() {
        let mut p = plugin("p", true, true);
        p.skills = vec![Default::default()];
        p.mcp_servers = vec![Default::default(), Default::default()];

        assert_eq!(summarize(&p), "1 skill · 2 MCP servers");
        assert_eq!(summarize(&plugin("empty", true, true)), "no components");
    }
}
