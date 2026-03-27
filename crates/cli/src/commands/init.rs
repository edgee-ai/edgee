use anyhow::Result;
use console::style;

setup_command! {}

pub async fn run(_opts: Options) -> Result<()> {
    crate::commands::auth::login::perform_login().await?;

    println!();
    println!(
        "  {} {}",
        style("All set!").bold().green(),
        style("Your Edgee credentials have been saved.").dim()
    );
    println!();

    Ok(())
}
