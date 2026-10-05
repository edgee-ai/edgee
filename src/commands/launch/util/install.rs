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
