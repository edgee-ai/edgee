use anyhow::Result;
use console::style;

setup_command! {}

pub async fn run(_opts: Options) -> Result<()> {
    let creds = crate::config::read()?;

    let has_any = creds.claude.as_ref().map(|c| !c.api_key.is_empty()).unwrap_or(false)
        || creds.codex.as_ref().map(|c| !c.api_key.is_empty()).unwrap_or(false);

    if !has_any {
        println!(
            "\n  {} {}\n",
            style("✗").red().bold(),
            style("Not logged in. Run `edgee auth login` to authenticate.").dim()
        );
        return Ok(());
    }

    println!();
    println!(
        "   {}  {}",
        style("Config:").dim(),
        style(crate::config::credentials_path().display()).dim()
    );

    for (name, provider) in [("Claude", &creds.claude), ("Codex", &creds.codex)] {
        if let Some(p) = provider.as_ref().filter(|p| !p.api_key.is_empty()) {
            println!();
            match &p.email {
                Some(e) if !e.is_empty() => println!("  {} {}", style("✓").green().bold(), style(format!("{name}: logged in as {e}")).bold()),
                _ => println!("  {} {}", style("✓").green().bold(), style(format!("{name}: logged in")).bold()),
            }
            if let Some(mode) = &p.connection {
                println!(
                    "   {}  {}",
                    style(format!("{name} mode:")).dim(),
                    style(mode).cyan()
                );
            }
        }
    }
    println!();

    Ok(())
}