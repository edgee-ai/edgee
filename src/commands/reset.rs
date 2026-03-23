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

    let provider = crate::commands::auth::login::prompt_provider()?;

    // Re-run login to get a new API key for the selected provider
    crate::commands::auth::login::perform_login(&provider).await?;

    // Re-prompt for connection mode and write back to the selected provider only
    let mut creds = crate::config::read()?;
    match provider.as_str() {
        "codex" => {
            let p = creds.codex.get_or_insert_with(Default::default);
            p.connection = Some("plan".to_string());
        }
        _ => {
            let choice = crate::commands::launch::claude::prompt_connection_mode()?;
            let p = creds.claude.get_or_insert_with(Default::default);
            p.connection = Some(choice);
        }
    }
    crate::config::write(&creds)?;

    println!(
        "  {} {}",
        style("Done!").bold().green(),
        style("Your credentials have been reset.").dim()
    );
    println!();

    Ok(())
}
