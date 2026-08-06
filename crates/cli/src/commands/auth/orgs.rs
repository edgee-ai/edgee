use anyhow::Result;
use console::style;
use serde::Serialize;

use crate::commands::util;

setup_command! {
    /// Switch the active profile's organization to this id or slug.
    #[arg(long)]
    pub set: Option<String>,
    /// Emit machine-readable JSON instead of the human-readable list.
    #[arg(long)]
    pub json: bool,
}

/// One organization in the `--json` output. Consumed by front-ends (the macOS
/// menubar app) to render an org switcher.
#[derive(Serialize)]
struct OrgEntry {
    id: String,
    slug: String,
    name: String,
    active: bool,
}

fn print_json(orgs: &[crate::api::Organization], active_id: Option<&str>) -> Result<()> {
    let entries: Vec<OrgEntry> = orgs
        .iter()
        .map(|o| OrgEntry {
            id: o.id.clone(),
            slug: o.slug.clone(),
            name: o.name.clone(),
            active: Some(o.id.as_str()) == active_id,
        })
        .collect();
    util::emit_json(&entries)
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Machine consumers get an empty list (exit 0) rather than an error when
    // there's no auth / no orgs, so the front-end can degrade gracefully.
    let Some(token) = creds.user_token.clone().filter(|t| !t.is_empty()) else {
        if opts.json {
            return util::emit_json(&Vec::<OrgEntry>::new());
        }
        anyhow::bail!("Not authenticated. Run `edgee auth login` first.");
    };

    let client = crate::api::ApiClient::new(&token)?;
    let orgs = client.list_organizations().await?;
    if orgs.is_empty() {
        if opts.json {
            return util::emit_json(&Vec::<OrgEntry>::new());
        }
        anyhow::bail!("No organizations found for this account.");
    }

    // --set: switch the active profile's org, then report.
    if let Some(target) = opts.set {
        let org = orgs
            .iter()
            .find(|o| o.id == target || o.slug == target)
            .ok_or_else(|| anyhow::anyhow!("Organization '{target}' not found"))?;
        creds.org_id = Some(org.id.clone());
        creds.org_slug = Some(org.slug.clone());
        crate::config::write(&creds)?;

        if opts.json {
            return print_json(&orgs, creds.org_id.as_deref());
        }
        println!(
            "\n  {} Now using organization {}.\n",
            style("✓").green().bold(),
            style(&org.name).bold()
        );
        return Ok(());
    }

    let active_id = creds.org_id.clone();
    if opts.json {
        return print_json(&orgs, active_id.as_deref());
    }

    println!();
    for org in &orgs {
        let marker = if Some(&org.id) == active_id.as_ref() {
            style("*").green().bold().to_string()
        } else {
            style(" ").dim().to_string()
        };
        println!("  {} {}  —  {}", marker, style(&org.name).bold(), org.slug);
    }
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Guards the field names the menubar app (Org in AuthStatus.swift) decodes.
    #[test]
    fn json_shape_is_stable() {
        let entry = OrgEntry {
            id: "abc".into(),
            slug: "acme".into(),
            name: "Acme".into(),
            active: true,
        };
        let v = serde_json::to_value(&entry).unwrap();
        for key in ["id", "slug", "name", "active"] {
            assert!(v.get(key).is_some(), "missing `{key}`");
        }
    }
}
