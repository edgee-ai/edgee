/// Declares all top-level subcommands and wires up dispatch.
///
/// Usage:
/// ```ignore
/// setup_commands! {
///     /// Doc comment shown in --help
///     CommandName(module_name),
/// }
/// ```
///
/// Expands to a `Command` enum and a `run()` async function.
macro_rules! setup_commands {
    ($(
        $([cfg($($cfg_tt:tt)*)])?
        $(#[$attr:meta])*
        $variant:ident($module:ident),
    )*) => {
        $($(#[cfg($($cfg_tt)*)])? pub mod $module;)*

        #[derive(Debug, clap::Subcommand)]
        pub enum Command {
            $(
                $(#[cfg($($cfg_tt)*)])?
                $(#[$attr])*
                $variant($module::Options),
            )*
        }

        pub async fn run(command: Command) -> anyhow::Result<()> {
            match command {
                $($(#[cfg($($cfg_tt)*)])? Command::$variant(opts) => $module::run(opts).await,)*
            }
        }
    };
}

/// Declares the `Options` struct for a single command module.
///
/// Usage:
/// ```ignore
/// setup_command! {
///     /// optional extra fields
/// }
/// ```
macro_rules! setup_command {
    () => {
        #[derive(Debug, clap::Parser)]
        pub struct Options {}
    };
    ($($field:tt)*) => {
        #[derive(Debug, clap::Parser)]
        pub struct Options {
            $($field)*
        }
    };
}
