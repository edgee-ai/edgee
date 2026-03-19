use anyhow::Result;
use console::style;

setup_command! {}

pub async fn run(_opts: Options) -> Result<()> {
    let provider = crate::commands::auth::login::prompt_provider()?;

    crate::commands::auth::login::perform_login(&provider).await?;

    let choice = match provider.as_str() {
        "codex" => crate::commands::launch::codex::prompt_connection_mode()?,
        _ => crate::commands::launch::claude::prompt_connection_mode()?,
    };

    let mut creds = crate::config::read()?;
    match provider.as_str() {
        "codex" => {
            let p = creds.codex.get_or_insert_with(Default::default);
            p.connection = Some(choice);
        }
        _ => {
            let p = creds.claude.get_or_insert_with(Default::default);
            p.connection = Some(choice);
        }
    }
    crate::config::write(&creds)?;

    println!();
    println!(
        "  {} {}",
        style("All set!").bold().green(),
        style("Your Edgee credentials have been saved.").dim()
    );
    println!();

    Ok(())
}
