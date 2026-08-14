//! `edgee plugins list` — what the org has assigned, and what each agent gets.

use anyhow::Result;
use console::style;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

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
    /// When the plugin last changed, phrased for a human.
    ///
    /// Where a version number used to be. A version said nothing anyone could
    /// act on — every machine runs the current one by construction, since a
    /// launch takes whatever the server has.
    pub edited: String,
    pub state: State,
    pub summary: String,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum State {
    /// Assigned to you: it is on this machine, and it is not yours to remove.
    Assigned,
    /// Visible only because you are an admin — it targets someone else.
    NotAssigned,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            State::Assigned => "assigned",
            State::NotAssigned => "not assigned",
        }
    }
}

/// "edited 3 days ago", from the server's RFC 3339 timestamp.
///
/// Degrades to a vague phrase rather than failing: a timestamp we cannot parse
/// is a cosmetic problem, and this is one column of a listing.
fn last_edited(updated_at: &str) -> String {
    let Ok(at) = OffsetDateTime::parse(updated_at, &Rfc3339) else {
        return "edited recently".to_string();
    };

    let elapsed = OffsetDateTime::now_utc() - at;
    let minutes = elapsed.whole_minutes();
    let hours = elapsed.whole_hours();
    let days = elapsed.whole_days();

    // Clock skew, or a plugin saved a moment ago: either way "just now" is true
    // enough and beats a negative duration.
    if minutes < 1 {
        return "edited just now".to_string();
    }
    if hours < 1 {
        return plural(minutes, "minute");
    }
    if days < 1 {
        return plural(hours, "hour");
    }
    if days < 30 {
        return plural(days, "day");
    }
    if days < 365 {
        return plural(days / 30, "month");
    }
    plural(days / 365, "year")
}

fn plural(n: i64, unit: &str) -> String {
    format!("edited {n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// Sorts active first, then what you could install, then the admin-only view.
/// Within a group, by name, so the listing is stable between runs.
pub fn rows(plugins: &[Plugin]) -> Vec<Row> {
    let mut rows: Vec<Row> = plugins
        .iter()
        .map(|p| Row {
            name: p.name.clone(),
            title: p.title().to_string(),
            edited: last_edited(&p.updated_at),
            state: state_of(p),
            summary: summarize(p),
        })
        .collect();
    rows.sort_by(|a, b| {
        let rank = |s: State| match s {
            State::Assigned => 0,
            State::NotAssigned => 1,
        };
        rank(a.state)
            .cmp(&rank(b.state))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

fn state_of(plugin: &Plugin) -> State {
    if plugin.active {
        State::Assigned
    } else {
        // Admins receive the whole org catalogue, so an untargeted plugin is not
        // an error — it just is not theirs.
        State::NotAssigned
    }
}

fn summarize(plugin: &Plugin) -> String {
    let counts = plugin.component_counts;
    let mut parts = Vec::new();
    for (n, one, many) in [
        (counts.skill, "skill", "skills"),
        (counts.subagent, "subagent", "subagents"),
        (counts.hook, "hook", "hooks"),
        (counts.mcp, "MCP server", "MCP servers"),
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
    // Metadata only: this command counts components and names them, and never
    // renders a skill body or a subagent prompt.
    let plugins = ApiClient::new(&token)?
        .list_plugins_metadata(&org_id)
        .await?;
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
            State::Assigned => (
                style("✓").green().bold(),
                style(row.state.label()).green(),
            ),
            State::NotAssigned => (style("·").dim(), style(row.state.label()).dim()),
        };
        println!(
            "  {glyph} {}  {}  {}  {}",
            style(&row.title).bold(),
            style(&row.edited).dim(),
            label,
            style(&row.summary).dim()
        );
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
                Kind::Skills => p.component_counts.skill > 0,
                Kind::Subagents => p.component_counts.subagent > 0,
                Kind::Hooks => p.component_counts.hook > 0,
                Kind::McpServers => p.component_counts.mcp > 0,
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
    use crate::api::PluginComponentCounts;

    fn plugin(name: &str, active: bool) -> Plugin {
        Plugin {
            id: format!("plg_{name}"),
            name: name.to_string(),
            active,
            ..Default::default()
        }
    }

    /// The listing degrades rather than breaks on a timestamp it cannot read —
    /// an unparseable date is one dim column, not a failed command.
    #[test]
    fn last_edited_falls_back_on_an_unreadable_timestamp() {
        assert_eq!(last_edited(""), "edited recently");
        assert_eq!(last_edited("yesterday-ish"), "edited recently");
    }

    /// A server clock slightly ahead of ours must not print a negative age.
    #[test]
    fn last_edited_reads_a_future_timestamp_as_just_now() {
        let ahead = OffsetDateTime::now_utc() + time::Duration::minutes(5);
        let formatted = ahead.format(&Rfc3339).unwrap();

        assert_eq!(last_edited(&formatted), "edited just now");
    }

    #[test]
    fn last_edited_picks_a_unit_and_singularizes() {
        let ago = |d: time::Duration| {
            last_edited(&(OffsetDateTime::now_utc() - d).format(&Rfc3339).unwrap())
        };

        assert_eq!(ago(time::Duration::seconds(20)), "edited just now");
        assert_eq!(ago(time::Duration::minutes(1)), "edited 1 minute ago");
        assert_eq!(ago(time::Duration::minutes(42)), "edited 42 minutes ago");
        assert_eq!(ago(time::Duration::hours(1)), "edited 1 hour ago");
        assert_eq!(ago(time::Duration::days(1)), "edited 1 day ago");
        assert_eq!(ago(time::Duration::days(9)), "edited 9 days ago");
        assert_eq!(ago(time::Duration::days(45)), "edited 1 month ago");
        assert_eq!(ago(time::Duration::days(400)), "edited 1 year ago");
    }

    #[test]
    fn rows_sort_assigned_first_then_the_admin_view() {
        let rows = rows(&[plugin("zeta-not-mine", false), plugin("alpha-mine", true)]);

        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha-mine", "zeta-not-mine"]
        );
        assert_eq!(rows[0].state, State::Assigned);
        assert_eq!(rows[1].state, State::NotAssigned);
    }

    #[test]
    fn rows_are_ordered_by_name_within_a_group() {
        let rows = rows(&[plugin("b", true), plugin("a", true)]);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    /// The summary reads `component_counts`, not the vectors: this command asks
    /// for the metadata view, where the vectors arrive empty by design.
    #[test]
    fn summary_counts_components_and_singularizes() {
        let mut p = plugin("p", true);
        p.component_counts = PluginComponentCounts {
            skill: 1,
            mcp: 2,
            ..Default::default()
        };

        assert_eq!(summarize(&p), "1 skill · 2 MCP servers");
        assert_eq!(summarize(&plugin("empty", true)), "no components");
    }
}
