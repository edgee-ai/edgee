use anyhow::Result;
use console::style;

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Log out of every profile, not just the active one.
    #[arg(long)]
    pub all: bool,
}

pub async fn run(opts: Options) -> Result<()> {
    if opts.all {
        let mut file = crate::config::read_file()?;
        for profile in file.profiles.values_mut() {
            clear_identity(profile);
        }
        crate::config::write_file(&file)?;
        println!(
            "  {} Logged out of all profiles.",
            style("✓").green().bold()
        );
    } else {
        let mut creds = crate::config::read()?;
        clear_identity(&mut creds);
        crate::config::write(&creds)?;
        println!(
            "  {} Logged out of profile {}.",
            style("✓").green().bold(),
            style(crate::config::active_profile_name()).bold()
        );
    }
    Ok(())
}

/// Clear a profile's identity (token, account, org) and its org-scoped provider
/// keys, leaving non-identity settings (URL overrides, MCP preference, debug
/// passphrase) intact.
fn clear_identity(profile: &mut crate::config::Profile) {
    profile.user_token = None;
    profile.email = None;
    profile.user_id = None;
    profile.org_id = None;
    profile.org_slug = None;
    profile.clear_provider_keys();
}
