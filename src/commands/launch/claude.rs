use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Extra args passed through to the claude CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we have an api_key
    if creds.claude.as_ref().map(|c| c.api_key.is_empty()).unwrap_or(true) {
        crate::commands::auth::login::perform_login("claude").await?;
        creds = crate::config::read()?;
    }

    // Step 2: ensure we have a connection choice
    if creds.claude.as_ref().and_then(|c| c.connection.as_deref()).is_none() {
        let choice = prompt_connection_mode()?;
        let provider = creds.claude.get_or_insert_with(Default::default);
        provider.connection = Some(choice);
        crate::config::write(&creds)?;
    }

    // Step 3: launch claude with the correct env vars
    let claude = creds.claude.as_ref().unwrap();
    let api_key = &claude.api_key;
    let mode = claude.connection.as_deref().unwrap_or("plan");
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut cmd = std::process::Command::new("claude");
    cmd.env("ANTHROPIC_BASE_URL", crate::config::api_base_url());

    match mode {
        "api" => {
            cmd.env("ANTHROPIC_AUTH_TOKEN", api_key);
            cmd.env("ANTHROPIC_API_KEY", "");
            cmd.env(
                "ANTHROPIC_CUSTOM_HEADERS",
                format!("x-edgee-session-id: {}", session_id),
            );
        }
        _ => {
            cmd.env(
                "ANTHROPIC_CUSTOM_HEADERS",
                format!("x-edgee-api-key: {}\nx-edgee-session-id: {}", api_key, session_id),
            );
        }
    }

    cmd.args(["--settings", r#"{"statusLine":{"type":"command","command":"printf 'Using \u001b[1;38;2;139;92;246mEdgee\u001b[0m to compress your tools'"}}"#]);
    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Claude Code is not installed. Install it from https://code.claude.com/docs/en/quickstart"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    {
        let logs_url = match creds.claude.as_ref().and_then(|c| c.org_slug.as_deref()) {
            Some(slug) if !slug.is_empty() => format!(
                "{}/~/{}/session/{}",
                crate::config::console_base_url(),
                slug,
                session_id
            ),
            _ => format!(
                "{}/~/me/session/{}",
                crate::config::console_base_url(),
                session_id
            ),
        };
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

pub fn prompt_connection_mode() -> Result<String> {
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
