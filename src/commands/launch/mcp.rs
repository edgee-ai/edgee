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

Your Edgee launch ID is: {session_id}
Your session page is: {session_url}

Always pass this launch ID as sessionId to the Edgee MCP tools below. Edgee maps it to your current conversation from the requests you have already sent, so a tool call must come after at least one model response in the current conversation, never before it. If a tool returns a session-not-available error, do not retry immediately: call it again at the end of a later response.

You MUST use the following Edgee MCP tools during this session:

1. `setSessionName` — call this at the END of your first response in the conversation (after you have done the work for the user's first message, as your last tool call before you reply), with a short descriptive name (3-6 words) summarizing what the user is asking for. Never call it before doing anything else. Arguments:
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
            "\n\n3. `setSessionGitRepo` — call this EXACTLY ONCE, together with (or right after) `setSessionName`. Arguments:\n   - sessionId: \"{session_id}\"\n   - repo: \"{repo}\"\n   Do not call this tool again during the session."
        ));
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_tools_run_after_the_first_response_not_before() {
        // The API maps the launch ID to the current conversation from usage, which
        // lands a few seconds after each request; naming first would hit the
        // previous conversation after /clear or /new.
        let prompt = session_instructions("launch-1", Some("owner/repo"), "https://console/s");
        assert!(prompt.contains("at the END of your first response"));
        assert!(!prompt.contains("immediately after the user's first message"));
        assert!(!prompt.contains("substitutes"));
        assert!(prompt.contains(r#"sessionId: "launch-1""#));
    }

    #[test]
    fn http_transport_authenticates_and_binds_current_launch() {
        let server = edgee_http_server("user-token", "launch-current");
        let wire = serde_json::to_string(&server).unwrap();
        let config: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(config["headers"]["Authorization"], "Bearer user-token");
        assert_eq!(config["headers"]["X-Edgee-Launch-Id"], "launch-current");
    }
}
