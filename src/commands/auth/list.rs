use anyhow::Result;
use console::style;

setup_command! {}

pub async fn run(_opts: Options) -> Result<()> {
    let file = crate::config::read_file()?;
    let active = crate::config::active_profile_name();

    if file.profiles.is_empty() {
        println!(
            "\n  {}\n",
            style("No profiles configured. Run `edgee auth login` to get started.").dim()
        );
        return Ok(());
    }

    println!();
    for (name, profile) in &file.profiles {
        let marker = if *name == active {
            style("*").green().bold().to_string()
        } else {
            style(" ").dim().to_string()
        };
        let email = profile.email.as_deref().unwrap_or("(not logged in)");
        let org = profile.org_slug.as_deref().unwrap_or("(no org)");
        println!("  {} {}  —  {} / {}", marker, style(name).bold(), email, org);
    }
    println!();

    Ok(())
}
