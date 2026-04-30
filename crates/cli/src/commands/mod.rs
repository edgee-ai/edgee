#[macro_use]
mod macros;

setup_commands! {
    /// Install shell aliases for Edgee launch commands
    Alias(alias),
    /// Authenticate with Edgee
    Auth(auth),
    /// Launch an AI tool routed through Edgee
    Launch(launch),
    /// Show stored session stats
    #[command(visible_alias = "report")]
    Stats(stats),
    /// Render the Edgee statusline (used by Claude Code's statusLine setting)
    Statusline(statusline),
    /// Diagnose Claude Code statusLine conflicts in the current project
    Doctor(doctor),
    /// Auto-overlay Edgee's statusline on top of a conflicting project setting
    Fix(fix),
    /// Install Edgee's user-level Claude Code integration (statusline, hooks)
    Install(install),
    /// Reset Edgee credentials and connection mode
    Reset(reset),
    [cfg(feature = "self-update")]
    /// Update Edgee
    SelfUpdate(update),
}
