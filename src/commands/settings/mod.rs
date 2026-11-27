use std::collections::BTreeMap;
use std::io::IsTerminal;

use anyhow::Result;
use clap::builder::PossibleValuesParser;
use clap::error::ErrorKind;
use clap::CommandFactory;
use dialoguer::{theme::ColorfulTheme, Select};

use crate::api::KeyCompression;
use crate::commands::auth::login;

pub mod agent;
mod headless;

/// Coding agents whose keys can be configured. Single source of truth for
/// the interactive picker and the `--agent` value parser below.
///
/// Deliberately distinct from `edgee launch`'s target list: launch uses
/// `copilot-vscode` for today's GitHub Copilot (VS Code) target, reserving
/// bare `copilot` for a future Copilot CLI, see `commands/launch/README.md`.
/// Both map to the same provider key here via `copilot`, which is why this
/// list uses the bare form.
const PROVIDERS: &[&str] = &[
    "claude",
    "claude_desktop",
    "codebuddy",
    "codex",
    "codex_desktop",
    "opencode",
    "crush",
    "pi",
    "deepseek",
    "kimi",
    "kilo",
    "cursor",
    "copilot",
];

/// A compression technique addressable from `--set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Technique {
    ToolResultTrimming,
    ToolSurfaceReduction,
    OutputBrevity,
}

impl Technique {
    pub const ALL: [Technique; 3] = [
        Technique::ToolResultTrimming,
        Technique::ToolSurfaceReduction,
        Technique::OutputBrevity,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Technique::ToolResultTrimming => "tool-result-trimming",
            Technique::ToolSurfaceReduction => "tool-surface-reduction",
            Technique::OutputBrevity => "output-brevity",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }

    pub fn get(self, c: &KeyCompression) -> Option<bool> {
        match self {
            Technique::ToolResultTrimming => c.tool_result_trimming,
            Technique::ToolSurfaceReduction => c.tool_surface_reduction,
            Technique::OutputBrevity => c.output_brevity,
        }
    }

    pub fn set(self, c: &mut KeyCompression, enabled: bool) {
        let slot = match self {
            Technique::ToolResultTrimming => &mut c.tool_result_trimming,
            Technique::ToolSurfaceReduction => &mut c.tool_surface_reduction,
            Technique::OutputBrevity => &mut c.output_brevity,
        };
        *slot = Some(enabled);
    }
}

/// One parsed `--set <technique>=<on|off>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetArg {
    pub technique: Technique,
    pub enabled: bool,
}

fn parse_set(raw: &str) -> Result<SetArg, String> {
    let names = || {
        Technique::ALL
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (name, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected <technique>=<on|off>, one of: {}", names()))?;
    let technique = Technique::from_name(name)
        .ok_or_else(|| format!("unknown technique `{name}`, expected one of: {}", names()))?;
    let enabled = match value {
        "on" => true,
        "off" => false,
        other => return Err(format!("expected `on` or `off` for {name}, got `{other}`")),
    };
    Ok(SetArg { technique, enabled })
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Coding agent whose key to configure.
    /// Prompts to pick one if omitted, in a terminal only.
    #[arg(value_parser = PossibleValuesParser::new(PROVIDERS.iter().copied()))]
    agent: Option<String>,

    /// Print the current settings as JSON instead of opening the wizard.
    #[arg(long)]
    json: bool,

    /// Turn a technique on or off without the wizard, e.g.
    /// `--set output-brevity=on`. Repeatable; techniques not named keep their
    /// current value. Techniques: tool-result-trimming, tool-surface-reduction,
    /// output-brevity.
    #[arg(long = "set", value_name = "TECHNIQUE=on|off", value_parser = parse_set)]
    set: Vec<SetArg>,
}

/// Whether to run the interactive wizard. Anything that asks for a machine
/// answer (`--json`, `--set`), or a missing terminal, selects the headless path,
/// since dialoguer fails with "not a terminal" when it cannot draw its prompts.
fn wants_wizard(json: bool, has_sets: bool, tty: bool) -> bool {
    tty && !json && !has_sets
}

/// Collapses repeated `--set`s into one value per technique. The same value
/// repeated is harmless; two different values for one technique are an error.
fn merge_sets(sets: &[SetArg]) -> Result<Vec<SetArg>, String> {
    let mut merged: BTreeMap<Technique, bool> = BTreeMap::new();
    for set in sets {
        if let Some(previous) = merged.insert(set.technique, set.enabled) {
            if previous != set.enabled {
                return Err(format!(
                    "--set {} was given both `on` and `off`",
                    set.technique.name()
                ));
            }
        }
    }
    Ok(merged
        .into_iter()
        .map(|(technique, enabled)| SetArg { technique, enabled })
        .collect())
}

/// Exits with a clap-style usage error (status 2), reported against `edgee settings`.
fn usage_error(kind: ErrorKind, message: impl std::fmt::Display) -> ! {
    let mut root = crate::Options::command();
    // Building propagates the `edgee` bin name into the usage line.
    root.build();
    match root.find_subcommand_mut("settings") {
        Some(settings) => settings.error(kind, message).exit(),
        None => root.error(kind, message).exit(),
    }
}

pub async fn run(opts: Options) -> Result<()> {
    // dialoguer draws on stderr and reads keys from stdin, so both must be terminals.
    let tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let wizard = wants_wizard(opts.json, !opts.set.is_empty(), tty);

    let sets = merge_sets(&opts.set)
        .unwrap_or_else(|msg| usage_error(ErrorKind::ArgumentConflict, msg));

    let target = match (opts.agent, wizard) {
        (Some(agent), _) => agent,
        (None, true) => prompt_for_target()?,
        (None, false) => usage_error(
            ErrorKind::MissingRequiredArgument,
            "the following required argument was not provided: <AGENT>\n\n  \
             The agent can only be picked interactively in a terminal.",
        ),
    };

    if wizard {
        // Reuse the auth flow's org gate so an unauthenticated user gets a clear hint.
        login::ensure_org_selected().await?;
        return agent::configure(&target, false).await;
    }

    headless::run(&target, &sets, opts.json).await
}

fn prompt_for_target() -> Result<String> {
    let items = PROVIDERS
        .iter()
        .map(|p| login::agent_label(p).to_string())
        .collect::<Vec<_>>();

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("What do you want to configure?")
        .items(&items)
        .default(0)
        .interact()?;

    Ok(PROVIDERS[selection].to_string())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    fn set(technique: Technique, enabled: bool) -> SetArg {
        SetArg { technique, enabled }
    }

    #[test]
    fn wizard_only_on_a_tty_without_machine_flags() {
        assert!(wants_wizard(false, false, true));
        assert!(!wants_wizard(true, false, true));
        assert!(!wants_wizard(false, true, true));
        assert!(!wants_wizard(false, false, false));
        assert!(!wants_wizard(true, true, false));
    }

    #[test]
    fn parse_set_accepts_every_technique_on_and_off() {
        for t in Technique::ALL {
            assert_eq!(parse_set(&format!("{}=on", t.name())), Ok(set(t, true)));
            assert_eq!(parse_set(&format!("{}=off", t.name())), Ok(set(t, false)));
        }
    }

    #[test]
    fn parse_set_rejects_malformed_input() {
        for bad in [
            "",
            "output-brevity",
            "=on",
            "output-brevity=",
            "output-brevity=true",
            "bogus=on",
            "Output-Brevity=on",
        ] {
            assert!(parse_set(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn merge_sets_dedups_equal_values_and_rejects_conflicts() {
        let a = set(Technique::OutputBrevity, true);
        assert_eq!(merge_sets(&[a, a]), Ok(vec![a]));
        assert!(merge_sets(&[a, set(Technique::OutputBrevity, false)]).is_err());
        let two = merge_sets(&[
            set(Technique::OutputBrevity, false),
            set(Technique::ToolResultTrimming, true),
        ])
        .unwrap();
        assert_eq!(two.len(), 2);
    }

    #[test]
    fn technique_set_touches_only_its_own_field() {
        let mut c = KeyCompression::default();
        Technique::ToolSurfaceReduction.set(&mut c, false);
        assert_eq!(c.tool_surface_reduction, Some(false));
        assert_eq!(c.tool_result_trimming, None);
        assert_eq!(c.output_brevity, None);
    }

    #[test]
    fn argv_parses_repeated_sets_and_json() {
        let opts = crate::Options::try_parse_from([
            "edgee",
            "settings",
            "claude",
            "--json",
            "--set",
            "output-brevity=on",
            "--set",
            "tool-result-trimming=off",
        ])
        .unwrap();
        let crate::commands::Command::Settings(s) = opts.command else {
            panic!("expected settings");
        };
        assert!(s.json);
        assert_eq!(s.agent.as_deref(), Some("claude"));
        assert_eq!(s.set.len(), 2);
    }

    #[test]
    fn argv_rejects_unknown_technique() {
        assert!(
            crate::Options::try_parse_from(["edgee", "settings", "claude", "--set", "bogus=on"])
                .is_err()
        );
    }
}
