use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Extra args passed through to the claude CLI
    #[arg(trailing_var_arg = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we have an api_key
    if creds.api_key.is_empty() {
        crate::commands::auth::login::perform_login().await?;
        creds = crate::config::read()?;
    }

    // Step 2: ensure we have a claude_connection choice
    if creds.claude_connection.is_none() {
        let choice = prompt_connection_mode()?;
        creds.claude_connection = Some(choice);
        crate::config::write(&creds)?;
    }

    // Step 3: launch claude with the correct env vars
    let mode = creds.claude_connection.as_deref().unwrap_or("plan");
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut cmd = std::process::Command::new("claude");
    cmd.env("ANTHROPIC_BASE_URL", crate::config::api_base_url());

    match mode {
        "api" => {
            cmd.env("ANTHROPIC_AUTH_TOKEN", &creds.api_key);
            cmd.env("ANTHROPIC_API_KEY", "");
            cmd.env(
                "ANTHROPIC_CUSTOM_HEADERS",
                format!("x-edgee-session-id: {}", session_id),
            );
        }
        _ => {
            cmd.env(
                "ANTHROPIC_CUSTOM_HEADERS",
                format!("x-edgee-api-key: {}\nx-edgee-session-id: {}", creds.api_key, session_id),
            );
        }
    }

    cmd.args(["--settings", r#"{"statusLine":{"type":"command","command":"printf 'Using \u001b[1;38;2;139;92;246mEdgee\u001b[0m to compress your tools'"}}"#]);
    cmd.args(&opts.args);

    let status = cmd.status()?;

    {
        let logs_url = format!("{}/me/session/{}", crate::config::console_base_url(), session_id);
        println!();
        println!(
            "  {} {}",
            style("Session ended.").bold(),
            style("Thanks for using Edgee + Claude!").dim()
        );
        println!(
            "  {} {}",
            style("View your Claude usage & compression stats at").dim(),
            style(&logs_url).cyan().underlined()
        );
        println!();
    }

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

fn prompt_connection_mode() -> Result<String> {
    println!();
    println!(
        "  {} How would you like to connect Claude Code to Edgee?",
        style("?").cyan().bold()
    );
    println!();

    let items = [
        style("Claude Pro/Max").green().bold().to_string(),
        style("API Billing").green().bold().to_string(),
    ];

    let selection = Select::with_theme(&ColorfulTheme::default())
        .items(&items)
        .default(0)
        .interact()?;

    println!();

    match selection {
        1 => Ok("api".to_string()),
        _ => Ok("plan".to_string()),
    }
}
