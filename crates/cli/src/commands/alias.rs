use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use console::style;

const MARKER_START: &str = "# >>> edgee launch aliases >>>";
const MARKER_END: &str = "# <<< edgee launch aliases <<<";
const CLAUDE_ALIAS: [AliasSpec; 1] = [AliasSpec::new("claude", "edgee launch claude")];
const CODEX_ALIAS: [AliasSpec; 1] = [AliasSpec::new("codex", "edgee launch codex")];
const OPENCODE_ALIAS: [AliasSpec; 1] = [AliasSpec::new("opencode", "edgee launch opencode")];
const ALL_ALIASES: [AliasSpec; 3] = [
    AliasSpec::new("claude", "edgee launch claude"),
    AliasSpec::new("codex", "edgee launch codex"),
    AliasSpec::new("opencode", "edgee launch opencode"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
    All,
}

impl Agent {
    fn aliases(self) -> &'static [AliasSpec] {
        match self {
            Self::Claude => &CLAUDE_ALIAS,
            Self::Codex => &CODEX_ALIAS,
            Self::Opencode => &OPENCODE_ALIAS,
            Self::All => &ALL_ALIASES,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::All => "claude, codex, and opencode",
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Remove installed Edgee aliases
    Remove {
        /// Which alias to remove
        #[arg(value_enum, default_value = "all")]
        agent: Agent,
    },
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    command: Option<Command>,

    /// Which alias to install
    #[arg(value_enum, default_value = "all")]
    agent: Agent,
}

pub async fn run(opts: Options) -> Result<()> {
    match opts.command {
        Some(Command::Remove { agent }) => apply_aliases(agent, Action::Remove),
        None => apply_aliases(opts.agent, Action::Install),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Install,
    Remove,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Self::Install => "installed",
            Self::Remove => "removed",
        }
    }
}

fn apply_aliases(agent: Agent, action: Action) -> Result<()> {
    let home = home_dir()?;
    let targets = [
        ShellConfig::new("bash", home.join(".bashrc"), ShellSyntax::Posix),
        ShellConfig::new("zsh", home.join(".zshrc"), ShellSyntax::Posix),
        ShellConfig::new(
            "fish",
            home.join(".config/fish/config.fish"),
            ShellSyntax::Fish,
        ),
    ];

    println!();
    for target in targets {
        let changed = match action {
            Action::Install => {
                upsert_managed_block(&target.path, &render_block(agent, target.syntax))?
            }
            Action::Remove => {
                remove_managed_block(&target.path, &render_block(agent, target.syntax))?
            }
        };
        let status = if changed {
            style("updated").green()
        } else {
            style("unchanged").dim()
        };
        println!(
            "  {} {} ({})",
            status,
            target.path.display(),
            style(target.shell).dim()
        );
    }
    println!();
    println!(
        "  {} {}",
        style(format!("Aliases {}.", action.verb())).bold().green(),
        style(format!("Affected: {}.", agent.label())).dim()
    );
    println!();

    Ok(())
}

#[derive(Clone, Copy)]
struct AliasSpec {
    name: &'static str,
    command: &'static str,
}

impl AliasSpec {
    const fn new(name: &'static str, command: &'static str) -> Self {
        Self { name, command }
    }
}

#[derive(Clone, Copy)]
enum ShellSyntax {
    Posix,
    Fish,
}

struct ShellConfig<'a> {
    shell: &'a str,
    path: PathBuf,
    syntax: ShellSyntax,
}

impl<'a> ShellConfig<'a> {
    fn new(shell: &'a str, path: PathBuf, syntax: ShellSyntax) -> Self {
        Self {
            shell,
            path,
            syntax,
        }
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("Could not determine your home directory from $HOME"))
}

fn render_block(agent: Agent, syntax: ShellSyntax) -> String {
    let mut block = String::from(MARKER_START);
    block.push('\n');
    for alias in agent.aliases() {
        match syntax {
            ShellSyntax::Posix => {
                block.push_str("alias ");
                block.push_str(alias.name);
                block.push_str("='");
                block.push_str(alias.command);
                block.push_str("'\n");
            }
            ShellSyntax::Fish => {
                block.push_str("alias ");
                block.push_str(alias.name);
                block.push_str(" '");
                block.push_str(alias.command);
                block.push_str("'\n");
            }
        }
    }
    block.push_str(MARKER_END);
    block.push('\n');
    block
}

fn upsert_managed_block(path: &Path, block: &str) -> Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let existing = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("Failed to read {}", path.display())),
    };

    let updated = replace_or_append_block(&existing, block)?;
    if updated == existing {
        return Ok(false);
    }

    std::fs::write(path, updated).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

fn remove_managed_block(path: &Path, block: &str) -> Result<bool> {
    let existing = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("Failed to read {}", path.display())),
    };

    let updated = remove_from_block(&existing, block)?;
    if updated == existing {
        return Ok(false);
    }

    std::fs::write(path, updated).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

fn replace_or_append_block(existing: &str, block: &str) -> Result<String> {
    match (existing.find(MARKER_START), existing.find(MARKER_END)) {
        (Some(start), Some(end)) if start <= end => {
            let after_end = end + MARKER_END.len();
            let suffix = existing[after_end..]
                .strip_prefix('\n')
                .unwrap_or(&existing[after_end..]);
            let mut updated = String::with_capacity(existing.len() + block.len());
            updated.push_str(&existing[..start]);
            updated.push_str(block);
            updated.push_str(suffix);
            Ok(updated)
        }
        (None, None) => {
            let mut updated = existing.to_string();
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            if !updated.is_empty() && !updated.ends_with("\n\n") {
                updated.push('\n');
            }
            updated.push_str(block);
            Ok(updated)
        }
        _ => anyhow::bail!(
            "Found a partial Edgee alias block. Please remove it from your shell config and try again."
        ),
    }
}

fn remove_from_block(existing: &str, block: &str) -> Result<String> {
    match (existing.find(MARKER_START), existing.find(MARKER_END)) {
        (Some(start), Some(end)) if start <= end => {
            let after_end = end + MARKER_END.len();
            let suffix = existing[after_end..]
                .strip_prefix('\n')
                .unwrap_or(&existing[after_end..]);
            let current_block = &existing[start..after_end + usize::from(existing[after_end..].starts_with('\n'))];
            let next_block = subtract_block(current_block, block);

            let mut updated = String::new();
            updated.push_str(&existing[..start]);
            updated.push_str(&next_block);
            updated.push_str(suffix);
            Ok(updated)
        }
        (None, None) => Ok(existing.to_string()),
        _ => anyhow::bail!(
            "Found a partial Edgee alias block. Please remove it from your shell config and try again."
        ),
    }
}

fn subtract_block(current: &str, to_remove: &str) -> String {
    let current_aliases = parse_alias_lines(current);
    let to_remove_aliases = parse_alias_lines(to_remove);
    let kept: Vec<&str> = current_aliases
        .into_iter()
        .filter(|line| !to_remove_aliases.contains(line))
        .collect();

    if kept.is_empty() {
        return String::new();
    }

    let mut updated = String::from(MARKER_START);
    updated.push('\n');
    for line in kept {
        updated.push_str(line);
        updated.push('\n');
    }
    updated.push_str(MARKER_END);
    updated.push('\n');
    updated
}

fn parse_alias_lines(block: &str) -> Vec<&str> {
    block
        .lines()
        .filter(|line| !line.is_empty() && *line != MARKER_START && *line != MARKER_END)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        remove_from_block, render_block, replace_or_append_block, Agent, ShellSyntax, MARKER_END,
        MARKER_START,
    };

    #[test]
    fn installs_single_alias_into_empty_file() {
        let block = render_block(Agent::Claude, ShellSyntax::Posix);
        let updated = replace_or_append_block("", &block).unwrap();
        assert_eq!(updated, block);
    }

    #[test]
    fn appends_block_after_existing_content() {
        let block = render_block(Agent::Opencode, ShellSyntax::Fish);
        let updated = replace_or_append_block("set -gx EDITOR vim\n", &block).unwrap();
        assert_eq!(updated, format!("set -gx EDITOR vim\n\n{block}"));
    }

    #[test]
    fn replaces_existing_block() {
        let old = format!("{MARKER_START}\nalias claude='old value'\n{MARKER_END}\n");
        let updated = replace_or_append_block(
            &format!("export PATH=x\n\n{old}"),
            &render_block(Agent::All, ShellSyntax::Posix),
        )
        .unwrap();
        assert!(updated.contains("alias codex='edgee launch codex'"));
    }

    #[test]
    fn removes_single_alias_from_multi_alias_block() {
        let all = render_block(Agent::All, ShellSyntax::Posix);
        let updated =
            remove_from_block(&all, &render_block(Agent::Codex, ShellSyntax::Posix)).unwrap();
        assert!(updated.contains("alias claude='edgee launch claude'"));
        assert!(!updated.contains("alias codex='edgee launch codex'"));
        assert!(updated.contains("alias opencode='edgee launch opencode'"));
    }

    #[test]
    fn removes_entire_block_when_last_alias_removed() {
        let block = render_block(Agent::Claude, ShellSyntax::Posix);
        let updated = remove_from_block(&block, &block).unwrap();
        assert_eq!(updated, "");
    }

    #[test]
    fn errors_on_partial_block() {
        let block = render_block(Agent::All, ShellSyntax::Posix);
        let err = replace_or_append_block(MARKER_START, &block).unwrap_err();
        assert!(err.to_string().contains("partial Edgee alias block"));
    }
}
