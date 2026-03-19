use anyhow::Result;
use console::style;

#[derive(Debug, clap::Parser)]
pub struct Options {}

pub async fn run(_opts: Options) -> Result<()> {
    println!();
    println!(
        "  {}",
        style("Resetting Edgee credentials…").bold()
    );
    println!();

    // Clear existing credentials (keep org_slug if present — it will be refreshed by login)
    let empty = crate::config::Credentials::default();
    crate::config::write(&empty)?;

    // Re-run login to get a new API key (also refreshes org_slug from callback)
    crate::commands::auth::login::perform_login().await?;

    // Re-prompt for connection mode
    let choice = crate::commands::launch::claude::prompt_connection_mode()?;
    let mut creds = crate::config::read()?;
    creds.claude_connection = Some(choice);
    crate::config::write(&creds)?;

    println!(
        "  {} {}",
        style("Done!").bold().green(),
        style("Your credentials have been reset.").dim()
    );
    println!();

    Ok(())
}
