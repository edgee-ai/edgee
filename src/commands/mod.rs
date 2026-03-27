#[macro_use]
mod macros;

setup_commands! {
    /// Initialize a new Edgee project
    Init(init),
    /// Authenticate with Edgee
    Auth(auth),
    /// Launch an AI tool routed through Edgee
    Launch(launch),
    /// Reset Edgee credentials and connection mode
    Reset(reset),
    /// Detect prompt efficiency issues in Claude Code sessions
    Suggest(suggest),
    [cfg(feature = "self-update")]
    /// Update Edgee
    SelfUpdate(update),
}
