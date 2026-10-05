//! Shared Edgee MCP wiring for CLI agents: the server entry and the
//! session-tracking instructions.

/// The `mcpServers.edgee` entry, in the `{type, url, headers}` shape both Claude
/// Code and Copilot CLI read. Authenticates with the console user token.
pub(super) fn edgee_http_server(token: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "http",
        "url": crate::config::mcp_base_url(),
        "headers": {
            "Authorization": format!("Bearer {token}")
        }
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

/// Console page for `session_id`, org-scoped when the org slug is known.
pub(super) fn session_url(creds: &crate::config::Credentials, session_id: &str) -> String {
    let base = crate::config::console_base_url();
    match creds.org_slug.as_deref().filter(|slug| !slug.is_empty()) {
        Some(slug) => format!("{base}/sessions/{slug}/{session_id}"),
        None => format!("{base}/sessions/{session_id}"),
    }
}

pub(super) fn session_instructions(session_id: &str, repo: Option<&str>, session_url: &str) -> String {
    let mut prompt = format!(
        r#"You are running inside the Edgee CLI and have access to the Edgee MCP server for tracking session metadata.

Your Edgee session ID is: {session_id}
Your Edgee public session page is: {session_url}

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
