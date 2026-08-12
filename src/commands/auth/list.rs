use anyhow::Result;
use console::style;
use serde::Serialize;

use crate::commands::util;

setup_command! {
    /// Emit machine-readable JSON instead of the human-readable list.
    #[arg(long)]
    pub json: bool,
}

/// One profile entry in the `--json` output. Consumed by front-ends (the macOS
/// menubar app) to render a profile switcher.
#[derive(Serialize)]
struct ProfileEntry {
    name: String,
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_slug: Option<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    let file = crate::config::read_file()?;
    let active = crate::config::active_profile_name();

    if opts.json {
        let entries: Vec<ProfileEntry> = file
            .profiles
            .iter()
            .map(|(name, profile)| ProfileEntry {
                name: name.clone(),
                active: *name == active,
                email: profile.email.clone().filter(|e| !e.is_empty()),
                org_slug: profile.org_slug.clone().filter(|s| !s.is_empty()),
            })
            .collect();
        return util::emit_json(&entries);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    // Guards the field names the menubar app (Profile in AuthStatus.swift) decodes.
    #[test]
    fn json_shape_is_stable() {
        let entry = ProfileEntry {
            name: "default".into(),
            active: true,
            email: Some("a@b.co".into()),
            org_slug: Some("acme".into()),
        };
        let v = serde_json::to_value(&entry).unwrap();
        for key in ["name", "active", "email", "org_slug"] {
            assert!(v.get(key).is_some(), "missing `{key}`");
        }
    }
}
