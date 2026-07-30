use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Name of the profile to switch to (interactive selector if omitted)
    pub name: Option<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    let mut file = crate::config::read_file()?;

    if file.profiles.is_empty() {
        anyhow::bail!("No profiles configured. Run `edgee auth login` to get started.");
    }

    let name = match opts.name {
        Some(n) => n,
        None => {
            let active = crate::config::active_profile_name();
            let names: Vec<&String> = file.profiles.keys().collect();
            let default_idx = names
                .iter()
                .position(|n| *n == &active)
                .unwrap_or(0);

            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Select profile")
                .items(&names)
                .default(default_idx)
                .interact()?;

            names[selection].clone()
        }
    };

    // Validate profile name: ascii alphanumeric, hyphens, underscores; max 64 chars.
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Invalid profile name `{name}`. Use letters, digits, hyphens, or underscores (max 64 chars)."
        );
    }

    if !file.profiles.contains_key(&name) {
        anyhow::bail!(
            "Profile `{name}` not found. Run `edgee auth list` to see available profiles,\nor `edgee auth login --profile {name}` to create it."
        );
    }

    file.active_profile = Some(name.clone());
    crate::config::write_file(&file)?;

    println!(
        "\n  {} Now using profile {}.\n",
        style("✓").green().bold(),
        style(&name).bold()
    );

    Ok(())
}
