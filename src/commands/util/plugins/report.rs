//! The one line a launch prints about plugin delivery.
//!
//! Shared by every target so the wording cannot drift between agents, and kept
//! in the launch path's existing quiet register: silent when there is nothing
//! to say, and never a per-plugin dump.

use console::style;

use super::sync::SyncReport;

/// One line about what the org's plugins delivered, in the launch path's
/// existing quiet register: silent when there is nothing to say.
pub fn report_launch(report: &SyncReport) {
    if report.is_empty() {
        return;
    }

    let plural = if report.plugin_count == 1 { "" } else { "s" };
    let summary = report.summary();
    let detail = if summary.is_empty() {
        String::new()
    } else {
        format!(" · {summary}")
    };
    println!(
        "  {} {}{}",
        style("✓").green().bold(),
        style(format!(
            "{} organization plugin{plural}",
            report.plugin_count
        ))
        .bold(),
        style(detail).dim()
    );

    if report.from_cache {
        println!(
            "  {}",
            style("⚠ Using cached organization plugins — could not reach Edgee.").yellow()
        );
    }

    // Named once, not per plugin: the launch path is not the place for a full
    // compatibility report, but silently dropping components is worse.
    if !report.undelivered.is_empty() {
        let kinds: Vec<&str> = report.undelivered.iter().map(|k| k.label()).collect();
        println!(
            "  {}",
            style(format!(
                "⚠ Not supported by this assistant: {} — see `edgee plugins list --verbose`",
                kinds.join(", ")
            ))
            .dim()
        );
    }
}
