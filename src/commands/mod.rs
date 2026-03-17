#[macro_use]
mod macros;

setup_commands! {
    /// Initialize a new Edgee project
    Init(init),
    /// Authenticate with Edgee
    Auth(auth),
    /// Launch an AI tool routed through Edgee
    Launch(launch),
    [cfg(feature = "self-update")]
    /// Update Edgee
    SelfUpdate(update),
}
