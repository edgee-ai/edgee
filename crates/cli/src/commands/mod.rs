#[macro_use]
mod macros;

pub mod claude_settings;

setup_commands! {
    /// Install shell aliases for Edgee launch commands
    Alias(alias),
    /// Authenticate with Edgee
    Auth(auth),
    /// Launch an AI tool routed through Edgee
    Launch(launch),
    /// Run a local HTTP gateway that forwards LLM requests through the Edgee pipeline
    ///
    /// Runs the gateway
    /// until Ctrl+C, then signals a graceful shutdown.
    ///
    /// Routes:
    ///   POST /v1/messages  → Anthropic Messages API (passthrough + compression)
    ///   POST /v1/responses → OpenAI Responses API   (passthrough + compression)
    ///
    /// Local dev only. No auth, no TLS, no rate limiting.
    LocalGateway(local_gateway),
    /// Show stored session stats
    #[command(visible_alias = "report")]
    Stats(stats),
    /// Render the Edgee statusline and manage agent statusline integrations
    Statusline(statusline),
    /// Reset Edgee credentials and connection mode
    Reset(reset),
    [cfg(feature = "self-update")]
    /// Update Edgee
    SelfUpdate(update),
}
