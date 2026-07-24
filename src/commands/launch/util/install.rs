use std::fs;

pub async fn ensure_first_run_installed() {
    use crate::commands::statusline::claude::{install, toggle};

    if toggle::is_disabled() {
        return;
    }

    // Always heal legacy/stale `statusLine.command` values on every launch
    // (silent and cheap). Covers users upgrading from older Edgee versions
    // whose `~/.claude/settings.json` still has the bare `edgee statusline`
    // form (which now prints help) or a wrapper-script path from the old
    // transient install. No-op if the field is already current or third-party.
    install::heal_legacy_statusline();

    let marker = toggle::installed_marker_path();
    if marker.is_file() {
        return;
    }

    if let Err(e) = install::run(install::Options::default()).await {
        eprintln!(
            "  {} edgee: skipped first-run statusline install: {e}",
            console::style("⚠").yellow()
        );
        return;
    }

    if let Some(parent) = marker.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&marker, b"");
}

pub fn spawn_cli_version_report(creds: &crate::config::Credentials, session_id: &str) {
    let token = creds
        .user_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    let org_id = creds
        .org_id
        .as_deref()
        .filter(|o| !o.is_empty())
        .map(str::to_owned);
    let (Some(token), Some(org_id)) = (token, org_id) else {
        return;
    };
    let session_id = session_id.to_owned();

    tokio::spawn(async move {
        if let Ok(client) = crate::api::ApiClient::new(&token) {
            let _ = client
                .set_session_cli_version(&org_id, &session_id, env!("CARGO_PKG_VERSION"))
                .await;
        }
    });
}
