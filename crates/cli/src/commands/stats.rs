use anyhow::{Result, bail};
use console::style;

setup_command! {
    /// Limit the number of sessions listed below the latest-session report
    #[arg(long)]
    pub limit: Option<usize>,
}

fn fmt_cost(nanodollars: u64) -> String {
    let dollars = nanodollars / 1_000_000_000;
    let frac = nanodollars % 1_000_000_000;
    format!("${}.{:09}", dollars, frac)
}

fn fmt_tokens(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

pub async fn run(opts: Options) -> Result<()> {
    let logs = crate::commands::launch::read_all_session_logs()?;
    if logs.is_empty() {
        bail!(
            "No stored session stats found in {}",
            crate::commands::launch::session_logs_dir().display()
        );
    }

    let latest = &logs[0];
    let total_requests: u64 = logs.iter().map(|entry| entry.stats.total_requests).sum();
    let total_cost: u64 = logs.iter().map(|entry| entry.stats.total_cost).sum();
    let total_savings: u64 = logs
        .iter()
        .map(|entry| entry.stats.total_token_cost_savings)
        .sum();
    let total_errors: u64 = logs.iter().map(|entry| entry.stats.total_errors).sum();
    let total_input_tokens: u64 = logs.iter().map(|entry| entry.stats.total_input_tokens).sum();
    let total_output_tokens: u64 = logs.iter().map(|entry| entry.stats.total_output_tokens).sum();

    println!();
    println!(
        "  {} {} stored sessions",
        style("Edgee stats").bold(),
        style(logs.len()).cyan()
    );
    println!(
        "  {} {}  {} {}  {} {}",
        style("Requests").bold().underlined(),
        style(total_requests).cyan(),
        style("Cost").bold().underlined(),
        style(fmt_cost(total_cost)).cyan(),
        style("Saved").bold().underlined(),
        style(fmt_cost(total_savings)).green(),
    );
    println!(
        "  {} {}  {} {}  {} {}",
        style("Input").bold().underlined(),
        style(fmt_tokens(total_input_tokens)).cyan(),
        style("Output").bold().underlined(),
        style(fmt_tokens(total_output_tokens)).cyan(),
        style("Errors").bold().underlined(),
        if total_errors > 0 {
            style(total_errors).red()
        } else {
            style(total_errors).green()
        },
    );

    println!();
    crate::commands::launch::render_session_stats(latest, Some("Latest session"));

    println!("  {}", style("All sessions").bold());
    println!();
    let limit = opts.limit.unwrap_or(logs.len()).max(1);
    for entry in logs.iter().take(limit) {
        let stats = &entry.stats;
        let compression = if stats.total_uncompressed_tools_tokens > 0
            && stats.total_compressed_tools_tokens < stats.total_uncompressed_tools_tokens
        {
            let pct = (stats.total_uncompressed_tools_tokens - stats.total_compressed_tools_tokens)
                * 100
                / stats.total_uncompressed_tools_tokens;
            style(format!("{pct}%")).green().to_string()
        } else {
            style("-").dim().to_string()
        };
        let errors = if stats.total_errors > 0 {
            style(stats.total_errors).red().to_string()
        } else {
            style("0").dim().to_string()
        };

        println!(
            "  {}  {}  {} req  {}  in {}  out {}  saved {}  cmp {}  err {}",
            style(&entry.ended_at).dim(),
            style(format!("{:<8}", entry.tool_name)).cyan(),
            style(format!("{:>4}", stats.total_requests)).cyan(),
            style(format!("{:>13}", fmt_cost(stats.total_cost))).cyan(),
            style(format!("{:>9}", fmt_tokens(stats.total_input_tokens))).cyan(),
            style(format!("{:>9}", fmt_tokens(stats.total_output_tokens))).cyan(),
            style(format!("{:>13}", fmt_cost(stats.total_token_cost_savings))).green(),
            compression,
            errors,
        );
        println!(
            "  {} {}",
            style("session").dim(),
            style(&entry.session_id).dim()
        );
    }
    println!();

    Ok(())
}
