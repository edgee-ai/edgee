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

    // Re-run login to get a new account token
    crate::commands::auth::login::perform_login().await?;

    println!(
        "  {} {}",
        style("Done!").bold().green(),
        style("Your credentials have been reset.").dim()
    );
    println!();

    Ok(())
}
