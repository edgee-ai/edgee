//! Shared Edgee MCP wiring for CLI agents: the server entry and the
//! session-tracking instructions.

/// User authentication and launch routing context.
pub(super) fn edgee_http_headers(token: &str, launch_id: &str) -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("X-Edgee-Launch-Id".to_string(), launch_id.to_string()),
    ])
}

/// Edgee HTTP server entry with explicit launch context.
pub(super) fn edgee_http_server(token: &str, launch_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "http",
        "url": crate::config::mcp_base_url(),
        "headers": edgee_http_headers(token, launch_id)
    })
}

/// Told to a member who opted into Edgee MCP when injection is off anyway.
/// Without this the integration would just silently vanish, which reads as a
/// bug rather than a deliberate setting. Names the actual source, so a
/// forgotten export doesn't look like an org decision.
pub(super) fn print_injection_skipped() {
    let reason = if crate::config::mcp_injection_disabled_env_override() == Some(true) {
        "EDGEE_MCP_INJECTION_DISABLED is set"
    } else {
        "Edgee MCP is turned off for your organization"
    };
    println!("{}", console::style(format!("  {reason}, skipping.")).dim());
}

/// Explicit launch-alias page, before the native identity is known.
pub(super) fn session_url(
    creds: &crate::config::Credentials,
    session_id: &str,
) -> anyhow::Result<String> {
    crate::config::console_launch_session_url(creds.org_slug.as_deref(), session_id)
}

pub(super) fn session_instructions(session_id: &str, repo: Option<&str>, session_url: &str) -> String {
    let mut prompt = format!(
        r#"You are running inside the Edgee CLI and have access to the Edgee MCP server for tracking session metadata.

Your Edgee launch ID (used for MCP only when no native agent session ID is available) is: {session_id}
Your launch-alias session page is: {session_url}

For current-session Edgee MCP calls, use the sessionId shown below. The gateway directly substitutes this agent's recognized native session ID whenever present, without session-association reads, writes or caches, including on-premise gateways. Explicit targets for different sessions remain unchanged. Native agent IDs are the canonical visible identity for API responses, session history, session links and MCP. The API resolves native IDs through existing usage records; first-call usage can arrive later than the tool call. If native-session lookup returns a session-not-available error, retry after usage arrives; never switch to the Edgee ID when a native ID exists. Use the Edgee ID only when no native ID is available. Do not guess a native ID, use a thread/cache ID, or create a separate session. The pre-launch page above remains an alias and resolves to the agent-facing session once usage arrives; prefer the API-returned session_id for later links. Resuming the same native ID groups launches together; a reset with a new native ID starts a separate visible session.

You MUST use the following Edgee MCP tools during this session:

1. `setSessionName` — call this immediately after the user's first message with a short descriptive name (3-6 words) summarizing what the user is asking for. Arguments:
   - sessionId: "{session_id}"
   - name: the descriptive name.
   If at any later point during the session you come up with a clearly better name (e.g., the task's real scope becomes obvious only after exploring the code, or the user pivots the request), call `setSessionName` again with the improved name. Prefer calling it once, but do not hesitate to update when a materially better name emerges.

2. `addSessionPullRequest` — call this EVERY TIME you create OR edit a pull request (e.g., via `gh pr create`, `gh pr edit`, or any other tool). Immediately after the PR is created or modified, call this tool with:
   - sessionId: "{session_id}"
   - pullRequest: the full PR URL.
   This is required for every PR you touch during this session, with no exceptions. Always call it on edits too — the PR may not yet be associated with this session, and the API handles duplicates safely, so redundant calls are harmless."#
    );

    if let Some(repo) = repo {
        prompt.push_str(&format!(
            "\n\n3. `setSessionGitRepo` — call this EXACTLY ONCE at the start of the session, together with (or right after) `setSessionName`. Arguments:\n   - sessionId: \"{session_id}\"\n   - repo: \"{repo}\"\n   Do not call this tool again during the session."
        ));
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_transport_authenticates_and_binds_current_launch() {
        let server = edgee_http_server("user-token", "launch-current");
        let wire = serde_json::to_string(&server).unwrap();
        let config: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(config["headers"]["Authorization"], "Bearer user-token");
        assert_eq!(config["headers"]["X-Edgee-Launch-Id"], "launch-current");
    }
}
