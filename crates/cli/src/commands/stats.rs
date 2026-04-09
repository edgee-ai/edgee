use anyhow::{Result, bail};
use console::style;

setup_command! {
    /// Limit the number of sessions listed below the latest-session report
    #[arg(long)]
    pub limit: Option<usize>,
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

fn fmt_compression_cell(before: u64, after: u64) -> (String, bool) {
    if before == 0 || after >= before {
        return (format!("{}  -", "░".repeat(8)), false);
    }

    let pct = (before - after) * 100 / before;
    let filled = (pct as usize * 8 / 100).min(8);
    let cell = format!("{}{} {:>2}%", "█".repeat(filled), "░".repeat(8 - filled), pct);
    (cell, true)
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
    let total_errors: u64 = logs.iter().map(|entry| entry.stats.total_errors).sum();
    let total_input_tokens: u64 = logs.iter().map(|entry| entry.stats.total_input_tokens).sum();
    let total_output_tokens: u64 = logs.iter().map(|entry| entry.stats.total_output_tokens).sum();

    println!();
    println!(
        "  {}  ·  {} sessions",
        style("Edgee stats").bold(),
        style(logs.len()).cyan()
    );
    println!();
    println!(
        "  {}  {}",
        style("Requests").bold().underlined(),
        style(total_requests).cyan(),
    );
    println!(
        "  {}     {}    {}  {}    {}  {}",
        style("In").bold().underlined(),
        style(fmt_tokens(total_input_tokens)).cyan(),
        style("Out").bold().underlined(),
        style(fmt_tokens(total_output_tokens)).cyan(),
        style("Errors").bold().underlined(),
        if total_errors > 0 {
            style(total_errors.to_string()).red()
        } else {
            style(total_errors.to_string()).dim()
        },
    );

    println!();
    crate::commands::launch::render_session_stats(latest, Some("Latest session"));

    println!("  {}", style("All sessions").bold());
    println!();
    let limit = opts.limit.unwrap_or(logs.len()).max(1);
    let visible_logs: Vec<_> = logs.iter().take(limit).collect();
    let tool_width = visible_logs
        .iter()
        .map(|entry| entry.tool_name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let req_width = visible_logs
        .iter()
        .map(|entry| entry.stats.total_requests.to_string().len())
        .max()
        .unwrap_or(3)
        .max(3);
    let in_width = visible_logs
        .iter()
        .map(|entry| fmt_tokens(entry.stats.total_input_tokens).len())
        .max()
        .unwrap_or(2)
        .max(2);
    let out_width = visible_logs
        .iter()
        .map(|entry| fmt_tokens(entry.stats.total_output_tokens).len())
        .max()
        .unwrap_or(3)
        .max(3);
    let err_width = visible_logs
        .iter()
        .map(|entry| entry.stats.total_errors.to_string().len())
        .max()
        .unwrap_or(3)
        .max(3);

    println!(
        "  {}  {}  {}  {}  {}  {}  {}",
        style(format!("{:<16}", "ended")).dim().bold(),
        style(format!("{:<tool_width$}", "tool")).dim().bold(),
        style(format!("{:>req_width$}", "req")).dim().bold(),
        style(format!("{:>in_width$}", "in")).dim().bold(),
        style(format!("{:>out_width$}", "out")).dim().bold(),
        style(format!("{:<12}", "compression")).dim().bold(),
        style(format!("{:>err_width$}", "err")).dim().bold(),
    );

    for entry in visible_logs {
        let stats = &entry.stats;
        let (compression, has_compression) = fmt_compression_cell(
            stats.total_uncompressed_tools_tokens,
            stats.total_compressed_tools_tokens,
        );
        let errors = stats.total_errors.to_string();

        println!(
            "  {}  {}  {}  {}  {}  {}  {}",
            style(crate::commands::launch::fmt_timestamp(&entry.ended_at)).dim(),
            style(format!("{:<tool_width$}", entry.tool_name)).cyan(),
            style(format!("{:>req_width$}", stats.total_requests)).cyan(),
            style(format!("{:>in_width$}", fmt_tokens(stats.total_input_tokens))).cyan(),
            style(format!("{:>out_width$}", fmt_tokens(stats.total_output_tokens))).cyan(),
            if has_compression {
                style(format!("{:<12}", compression)).green()
            } else {
                style(format!("{:<12}", compression)).dim()
            },
            if stats.total_errors > 0 {
                style(format!("{:>err_width$}", errors)).red()
            } else {
                style(format!("{:>err_width$}", errors)).dim()
            },
        );
    }
    println!();

    Ok(())
}
