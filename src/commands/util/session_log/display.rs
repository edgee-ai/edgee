use console::style;

use super::log::SessionLogEntry;

pub fn fmt_tokens(n: u64) -> String {
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

pub fn fmt_timestamp(ts: &str) -> String {
    if ts.len() >= 16 {
        ts[..16].replace('T', " ")
    } else {
        ts.to_string()
    }
}

pub fn compression_pct(before: u64, after: u64) -> u64 {
    if before == 0 {
        return 0;
    }
    (before - after) * 100 / before
}

pub fn pad_left(s: &str, width: usize) -> String {
    format!("{s:>width$}")
}

pub fn pad_right(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

pub fn fmt_bar(pct: u64, width: usize) -> String {
    let filled = (pct as usize * width / 100).min(width);
    let empty = width - filled;
    format!(
        "{}{}",
        style("█".repeat(filled)).green(),
        style("░".repeat(empty)).dim()
    )
}

pub fn render_session_stats(entry: &SessionLogEntry, heading: Option<&str>) {
    if let Some(heading) = heading {
        println!("  {}", style(heading).bold());
        println!();
    }

    println!(
        "  {} {}  {} {}",
        style("Tool").bold().underlined(),
        style(&entry.tool_name).cyan(),
        style("Ended").bold().underlined(),
        style(fmt_timestamp(&entry.ended_at)).dim(),
    );
    println!(
        "  {} {}",
        style("Session").bold().underlined(),
        style(&entry.session_id).dim()
    );

    let stats = &entry.stats;

    println!();
    let error_note = if stats.total_errors > 0 {
        format!("  ·  {} errors", style(stats.total_errors).red())
    } else {
        String::new()
    };
    println!(
        "  {}  {} requests{}",
        style("Overview").bold().underlined(),
        style(stats.total_requests).cyan(),
        error_note,
    );

    println!();
    println!("  {}", style("Tokens").bold().underlined());

    let mut input_detail = String::new();
    if stats.total_cached_input_tokens > 0 {
        input_detail.push_str(&format!(
            "  cached: {}",
            style(fmt_tokens(stats.total_cached_input_tokens)).dim()
        ));
    }
    if stats.total_cache_creation_input_tokens > 0 {
        input_detail.push_str(&format!(
            "  cache-write: {}",
            style(fmt_tokens(stats.total_cache_creation_input_tokens)).dim()
        ));
    }
    println!(
        "  Input   {}{}",
        style(fmt_tokens(stats.total_input_tokens)).cyan(),
        input_detail,
    );

    let reasoning_note = if stats.total_reasoning_output_tokens > 0 {
        format!(
            "  reasoning: {}",
            style(fmt_tokens(stats.total_reasoning_output_tokens)).dim()
        )
    } else {
        String::new()
    };
    println!(
        "  Output  {}{}",
        style(fmt_tokens(stats.total_output_tokens)).cyan(),
        reasoning_note,
    );

    let has_tool_compression = stats.total_uncompressed_tools_tokens > 0
        && stats.total_compressed_tools_tokens < stats.total_uncompressed_tools_tokens;

    if has_tool_compression {
        println!();
        println!("  {}", style("Compression").bold().underlined());

        let pct = compression_pct(
            stats.total_uncompressed_tools_tokens,
            stats.total_compressed_tools_tokens,
        );
        println!(
            "  Tools   {} -> {}  {} {}% saved",
            style(fmt_tokens(stats.total_uncompressed_tools_tokens)).dim(),
            style(fmt_tokens(stats.total_compressed_tools_tokens)).cyan(),
            fmt_bar(pct, 20),
            style(pct).green(),
        );

        if let Some(tool_stats) = &stats.tool_compression_stats {
            if !tool_stats.is_empty() {
                let mut tools: Vec<_> = tool_stats.iter().collect();
                tools.sort_by_key(|b| std::cmp::Reverse(b.1.before));
                println!();
                println!("  {}", style("Tool breakdown").bold().underlined());
                println!(
                    "  {} {} {} Savings",
                    pad_right("Tool", 20),
                    pad_left("Calls", 5),
                    pad_right("Tokens", 20)
                );
                for (name, ts) in &tools {
                    let pct = compression_pct(ts.before, ts.after);
                    println!(
                        "  {} {} {} -> {} {} {}% saved",
                        style(pad_right(name.as_str(), 20)).cyan(),
                        pad_left(&ts.count.to_string(), 5),
                        style(pad_left(&fmt_tokens(ts.before), 9)).dim(),
                        style(pad_left(&fmt_tokens(ts.after), 9)).cyan(),
                        fmt_bar(pct, 10),
                        style(pct).green(),
                    );
                }
            }
        }
    }

    println!();
    println!(
        "  {} {}",
        style("Full details at").dim(),
        style(&entry.logs_url).cyan().underlined()
    );
    println!();
}
