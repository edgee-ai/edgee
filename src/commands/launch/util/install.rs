use std::time::Duration;

use crate::{api::ApiClient, config::Credentials};

/// Register the session before the agent can open its model picker. Registration
/// failures must not prevent otherwise usable gateway credentials from launching.
pub async fn report_cli_session(creds: &Credentials, session_id: &str, provider: &str) {
    let token = creds.user_token.as_deref().filter(|t| !t.is_empty());
    let org_id = creds.org_id.as_deref().filter(|o| !o.is_empty());
    let (Some(token), Some(org_id)) = (token, org_id) else {
        return;
    };
    let key_id = creds
        .provider(provider)
        .and_then(|p| p.api_key_id.as_deref())
        .filter(|id| !id.is_empty());
    let report = async {
        ApiClient::new(token)?
            .set_session_cli_version(org_id, session_id, env!("CARGO_PKG_VERSION"), key_id)
            .await
    };
    // Bound startup latency if the control plane is unavailable. Older clients
    // without a key ID retain version-only reporting and traffic-based lookup.
    let result = tokio::time::timeout(Duration::from_secs(5), report).await;
    if !matches!(result, Ok(Ok(()))) {
        eprintln!("Warning: Edgee session registration failed. Model selection may be unavailable until traffic is recorded; retry /edgee list after your first request.");
    }
}
