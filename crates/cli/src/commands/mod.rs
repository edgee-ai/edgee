#[macro_use]
mod macros;

setup_commands! {
    /// Initialize a new Edgee project
    Init(init),
    /// Install shell aliases for Edgee launch commands
    Alias(alias),
    /// Authenticate with Edgee
    Auth(auth),
    /// Launch an AI tool routed through Edgee
    Launch(launch),
    /// Show stored session stats
    #[command(visible_alias = "report")]
    Stats(stats),
    /// Reset Edgee credentials and connection mode
    Reset(reset),
    [cfg(feature = "self-update")]
    /// Update Edgee
    SelfUpdate(update),
}
