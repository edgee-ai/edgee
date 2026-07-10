#[macro_use]
mod macros;

pub mod claude_settings;
pub(crate) mod util;

setup_commands! {
    /// Install shell aliases for Edgee launch commands
    Alias(alias),
    /// Authenticate with Edgee
    Auth(auth),
    /// Launch an AI tool routed through Edgee
    Launch(launch),
    /// Configure compression, fallback, and reroute settings for a coding-agent key
    Settings(settings),
    /// Show stored session stats
    #[command(visible_alias = "report")]
    Stats(stats),
    /// Render the Edgee statusline and manage agent statusline integrations
    Statusline(statusline),
    [cfg(feature = "relay")]
    /// Relay LLM API traffic through the Edgee gateway via a local MITM proxy
    #[command(hide = true)]
    Relay(relay),
    /// Reset Edgee credentials and connection mode
    Reset(reset),
    [cfg(feature = "self-update")]
    /// Update Edgee
    #[command(visible_alias = "self-update")]
    Update(update),

}
