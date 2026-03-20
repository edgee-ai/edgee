use anyhow::Result;
use console::style;

setup_command! {}

pub async fn run(_opts: Options) -> Result<()> {
    let creds = crate::config::read()?;

    if creds.api_key.is_empty() {
        println!(
            "\n  {} {}\n",
            style("✗").red().bold(),
            style("Not logged in. Run `edgee auth login` to authenticate.").dim()
        );
        return Ok(());
    }

    println!();
    match creds.email {
        Some(e) => println!("  {} {}", style("✓").green().bold(), style(format!("Logged in as {e}")).bold()),
        None    => println!("  {} {}", style("✓").green().bold(), style("Logged in").bold()),
    }
    println!(
        "   {}  {}",
        style("Config:").dim(),
        style(crate::config::credentials_path().display()).dim()
    );

    if let Some(mode) = &creds.claude_connection {
        println!();
        println!(
            "   {}  {}",
            style("Claude Code mode:").dim(),
            style(mode).cyan()
        );
    }
    println!();

    Ok(())
}