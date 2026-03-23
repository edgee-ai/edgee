use anyhow::Result;
use console::style;

setup_command! {}

pub async fn run(_opts: Options) -> Result<()> {
    let provider = crate::commands::auth::login::prompt_provider()?;

    crate::commands::auth::login::perform_login(&provider).await?;

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

    println!();
    println!(
        "  {} {}",
        style("All set!").bold().green(),
        style("Your Edgee credentials have been saved.").dim()
    );
    println!();

    Ok(())
}
