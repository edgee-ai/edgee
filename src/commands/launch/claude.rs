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

    let mut cmd = std::process::Command::new("claude");
    cmd.env("ANTHROPIC_BASE_URL", crate::config::api_base_url());

    match mode {
        "api" => {
            cmd.env("ANTHROPIC_AUTH_TOKEN", &creds.api_key);
            cmd.env("ANTHROPIC_API_KEY", "");
        }
        _ => {
            cmd.env(
                "ANTHROPIC_CUSTOM_HEADERS",
                format!("x-edgee-api-key:{}", creds.api_key),
            );
        }
    }

    cmd.args(&opts.args);

    let status = cmd.status()?;
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
        format!(
            "{}  {}",
            style("Claude Pro/Max").green().bold(),
            style("· uses ANTHROPIC_CUSTOM_HEADERS").dim()
        ),
        format!(
            "{}      {}",
            style("API Billing").green().bold(),
            style("· uses ANTHROPIC_AUTH_TOKEN").dim()
        ),
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
