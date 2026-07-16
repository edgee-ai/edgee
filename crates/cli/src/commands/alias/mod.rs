//! `edgee alias` — install CLI PATH shims / shell aliases **and** desktop app
//! wrappers for GUI launch targets. See [`desktop`] and
//! `crates/cli/src/commands/launch/README.md`.

mod desktop;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use console::style;

use desktop::{AppSpec, ALL_APPS, COPILOT_VSCODE_APP, CURSOR_APP};

const MARKER_START: &str = "# >>> edgee launch aliases >>>";
const MARKER_END: &str = "# <<< edgee launch aliases <<<";

const SHIM_DIR_REL: &str = ".edgee/bin";

// Unix routes via PATH shims; elsewhere via shell aliases. Never both for one
// name — alias + shim-on-PATH double-wraps the launch.
const USES_SHIMS: bool = cfg!(unix);

const CLAUDE_ALIAS: AliasSpec = AliasSpec::new("claude", "edgee launch claude");
const CODEBUDDY_ALIAS: AliasSpec = AliasSpec::new("codebuddy", "edgee launch codebuddy");
const CODEX_ALIAS: AliasSpec = AliasSpec::new("codex", "edgee launch codex");
const OPENCODE_ALIAS: AliasSpec = AliasSpec::new("opencode", "edgee launch opencode");
const CRUSH_ALIAS: AliasSpec = AliasSpec::new("crush", "edgee launch crush");

const ALL_ALIASES: [AliasSpec; 5] = [
    CLAUDE_ALIAS,
    CODEBUDDY_ALIAS,
    CODEX_ALIAS,
    OPENCODE_ALIAS,
    CRUSH_ALIAS,
];

const PATH_EXPORT_POSIX: &str = "case \":$PATH:\" in\n  *\":$HOME/.edgee/bin:\"*) ;;\n  *) export PATH=\"$HOME/.edgee/bin:$PATH\" ;;\nesac\n";
const PATH_EXPORT_FISH: &str = "fish_add_path -p \"$HOME/.edgee/bin\"\n";

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codebuddy,
    Codex,
    Opencode,
    Crush,
    /// Cursor IDE desktop wrapper (requires Cursor installed)
    Cursor,
    /// GitHub Copilot in VS Code desktop wrapper (requires VS Code installed)
    #[value(name = "copilot-vscode")]
    CopilotVscode,
    All,
}

impl Agent {
    /// CLI targets → PATH shims / shell aliases.
    fn aliases(self) -> &'static [AliasSpec] {
        match self {
            Self::Claude => std::slice::from_ref(&CLAUDE_ALIAS),
            Self::Codebuddy => std::slice::from_ref(&CODEBUDDY_ALIAS),
            Self::Codex => std::slice::from_ref(&CODEX_ALIAS),
            Self::Opencode => std::slice::from_ref(&OPENCODE_ALIAS),
            Self::Crush => std::slice::from_ref(&CRUSH_ALIAS),
            Self::Cursor | Self::CopilotVscode => &[],
            Self::All => &ALL_ALIASES,
        }
    }

    /// GUI targets → desktop app wrappers (see [`desktop`]).
    fn apps(self) -> &'static [AppSpec] {
        match self {
            Self::Cursor => std::slice::from_ref(&CURSOR_APP),
            Self::CopilotVscode => std::slice::from_ref(&COPILOT_VSCODE_APP),
            Self::All => ALL_APPS,
            _ => &[],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codebuddy => "codebuddy",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Crush => "crush",
            Self::Cursor => "cursor",
            Self::CopilotVscode => "copilot-vscode",
            Self::All => "claude, codebuddy, codex, opencode, crush, cursor, and copilot-vscode",
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
    let shim_dir = home.join(SHIM_DIR_REL);
    let cli = agent.aliases();

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

    // CLI agents → PATH shims + shell rc block.
    if !cli.is_empty() {
        if USES_SHIMS {
            match action {
                Action::Install => write_shims(&shim_dir, cli)?,
                Action::Remove => remove_shims(&shim_dir, cli)?,
            }
            println!(
                "  {} {} ({})",
                style("updated").green(),
                shim_dir.display(),
                style("shims").dim()
            );
        }

        for target in &targets {
            let changed = sync_target(target, &shim_dir, agent, action)?;
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
    }

    // GUI agents → desktop wrappers (only if the host app is installed).
    let desktop_action = match action {
        Action::Install => desktop::Action::Install,
        Action::Remove => desktop::Action::Remove,
    };
    desktop::apply_apps(agent.apps(), desktop_action)?;

    println!();
    println!(
        "  {} {}",
        style(format!("Aliases {}.", action.verb())).bold().green(),
        style(format!("Affected: {}.", agent.label())).dim()
    );

    if matches!(action, Action::Install) && USES_SHIMS && !cli.is_empty() {
        println!();
        println!(
            "  {} Reopen your terminal (or `exec $SHELL -l`) so `$HOME/.edgee/bin` lands on PATH.",
            style("Note:").bold()
        );
    }
    if matches!(action, Action::Install) && !agent.apps().is_empty() {
        println!();
        println!(
            "  {} App wrappers appear in ~/Applications (macOS), your app menu (Linux), or the Start Menu (Windows). Desktop wrappers are skipped when the target app is not installed.",
            style("Note:").bold()
        );
    }
    println!();

    Ok(())
}

// Reconcile one shell's rc file. On unix the block only puts the shim dir on
// PATH, so keep it while any shim remains; elsewhere edit the alias lines.
fn sync_target(
    target: &ShellConfig,
    shim_dir: &Path,
    agent: Agent,
    action: Action,
) -> Result<bool> {
    if USES_SHIMS {
        let keep = match action {
            Action::Install => true,
            Action::Remove => any_shims_remain(shim_dir),
        };
        if keep {
            upsert_managed_block(&target.path, &render_block(agent.aliases(), target.syntax))
        } else {
            remove_managed_block(&target.path)
        }
    } else {
        match action {
            Action::Install => {
                let block = render_block(agent.aliases(), target.syntax);
                upsert_managed_block(&target.path, &block)
            }
            Action::Remove => {
                remove_aliases_from_file(&target.path, agent.aliases(), target.syntax)
            }
        }
    }
}

fn any_shims_remain(shim_dir: &Path) -> bool {
    ALL_ALIASES
        .iter()
        .any(|spec| shim_dir.join(spec.name).exists())
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

fn render_block(aliases: &[AliasSpec], syntax: ShellSyntax) -> String {
    if USES_SHIMS {
        render_path_export_block(syntax)
    } else {
        render_alias_block(aliases, syntax)
    }
}

// Unix block: just put the shim dir on PATH (the shims do the routing).
fn render_path_export_block(syntax: ShellSyntax) -> String {
    let path_snippet = match syntax {
        ShellSyntax::Posix => PATH_EXPORT_POSIX,
        ShellSyntax::Fish => PATH_EXPORT_FISH,
    };
    format!("{MARKER_START}\n{path_snippet}{MARKER_END}\n")
}

// Non-unix block: shell aliases only (no shim dir to add to PATH).
fn render_alias_block(aliases: &[AliasSpec], syntax: ShellSyntax) -> String {
    let mut block = String::from(MARKER_START);
    block.push('\n');
    for alias in aliases {
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

fn remove_aliases_from_file(
    path: &Path,
    removing: &[AliasSpec],
    syntax: ShellSyntax,
) -> Result<bool> {
    let existing = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("Failed to read {}", path.display())),
    };

    let updated = subtract_aliases_from_text(&existing, removing, syntax)?;
    if updated == existing {
        return Ok(false);
    }

    std::fs::write(path, updated).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

// Drop the whole managed block (used on unix when the last shim is removed).
fn remove_managed_block(path: &Path) -> Result<bool> {
    let existing = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("Failed to read {}", path.display())),
    };

    let updated = strip_managed_block(&existing)?;
    if updated == existing {
        return Ok(false);
    }

    std::fs::write(path, updated).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

fn strip_managed_block(existing: &str) -> Result<String> {
    match (existing.find(MARKER_START), existing.find(MARKER_END)) {
        (Some(start), Some(end)) if start <= end => {
            let after_end = end + MARKER_END.len();
            let suffix = existing[after_end..]
                .strip_prefix('\n')
                .unwrap_or(&existing[after_end..]);
            let mut updated = String::with_capacity(existing.len());
            updated.push_str(&existing[..start]);
            updated.push_str(suffix);
            Ok(updated)
        }
        (None, None) => Ok(existing.to_string()),
        _ => anyhow::bail!(
            "Found a partial Edgee alias block. Please remove it from your shell config and try again."
        ),
    }
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

fn subtract_aliases_from_text(
    existing: &str,
    removing: &[AliasSpec],
    syntax: ShellSyntax,
) -> Result<String> {
    match (existing.find(MARKER_START), existing.find(MARKER_END)) {
        (Some(start), Some(end)) if start <= end => {
            let after_end = end + MARKER_END.len();
            let suffix = existing[after_end..]
                .strip_prefix('\n')
                .unwrap_or(&existing[after_end..]);

            let current_block_end = after_end + usize::from(existing[after_end..].starts_with('\n'));
            let current_block = &existing[start..current_block_end];

            let present_specs: Vec<AliasSpec> = ALL_ALIASES
                .iter()
                .copied()
                .filter(|spec| block_contains_alias(current_block, spec.name))
                .collect();

            let remaining: Vec<AliasSpec> = present_specs
                .into_iter()
                .filter(|spec| !removing.iter().any(|r| r.name == spec.name))
                .collect();

            let next_block = if remaining.is_empty() {
                String::new()
            } else {
                render_alias_block(&remaining, syntax)
            };

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

fn block_contains_alias(block: &str, name: &str) -> bool {
    let posix = format!("alias {name}=");
    let fish = format!("alias {name} ");
    block.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with(&posix) || trimmed.starts_with(&fish)
    })
}

#[cfg(unix)]
fn write_shims(shim_dir: &Path, aliases: &[AliasSpec]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(shim_dir)
        .with_context(|| format!("Failed to create {}", shim_dir.display()))?;

    for alias in aliases {
        let path = shim_dir.join(alias.name);
        let body = render_shim_script(alias.command);

        let tmp = path.with_extension("tmp");
        {
            let mut f = std::fs::File::create(&tmp)
                .with_context(|| format!("Failed to create {}", tmp.display()))?;
            f.write_all(body.as_bytes())
                .with_context(|| format!("Failed to write {}", tmp.display()))?;
            f.set_permissions(std::fs::Permissions::from_mode(0o755))
                .with_context(|| format!("Failed to chmod {}", tmp.display()))?;
        }
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("Failed to install shim at {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_shims(_shim_dir: &Path, _aliases: &[AliasSpec]) -> Result<()> {
    // PATH shims are POSIX-only for now; shell aliases remain available.
    Ok(())
}

#[cfg(unix)]
fn remove_shims(shim_dir: &Path, aliases: &[AliasSpec]) -> Result<()> {
    for alias in aliases {
        let path = shim_dir.join(alias.name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("Failed to remove shim at {}", path.display()))
            }
        }
    }
    // Best-effort: prune empty shim dir.
    let _ = std::fs::remove_dir(shim_dir);
    Ok(())
}

#[cfg(not(unix))]
fn remove_shims(_shim_dir: &Path, _aliases: &[AliasSpec]) -> Result<()> {
    Ok(())
}

fn render_shim_script(launch_command: &str) -> String {
    format!(
        "#!/usr/bin/env bash\n\
# Edgee shim — routes this binary through the Edgee Gateway.\n\
# Managed by `edgee alias`. Do not edit by hand.\n\
PATH=$(printf ':%s:' \"$PATH\" | sed -e \"s|:$HOME/.edgee/bin:|:|g\" -e \"s|^:||\" -e \"s|:$||\")\n\
export PATH\n\
exec {launch_command} \"$@\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_only() -> Vec<AliasSpec> {
        vec![CLAUDE_ALIAS]
    }

    fn codex_only() -> Vec<AliasSpec> {
        vec![CODEX_ALIAS]
    }

    #[test]
    fn installs_single_alias_into_empty_file() {
        let block = render_block(&claude_only(), ShellSyntax::Posix);
        let updated = replace_or_append_block("", &block).unwrap();
        assert_eq!(updated, block);
    }

    #[test]
    fn path_export_block_has_no_alias_lines() {
        let posix = render_path_export_block(ShellSyntax::Posix);
        assert!(posix.contains("case \":$PATH:\" in"));
        assert!(posix.contains("$HOME/.edgee/bin:$PATH"));
        assert!(!posix.contains("alias "));

        let fish = render_path_export_block(ShellSyntax::Fish);
        assert!(fish.contains("fish_add_path -p \"$HOME/.edgee/bin\""));
        assert!(!fish.contains("alias "));
    }

    #[test]
    fn alias_block_has_no_path_export() {
        let posix = render_alias_block(&ALL_ALIASES, ShellSyntax::Posix);
        assert!(posix.contains("alias claude='edgee launch claude'"));
        assert!(posix.contains("alias codex='edgee launch codex'"));
        assert!(!posix.contains("$HOME/.edgee/bin"));

        let fish = render_alias_block(&ALL_ALIASES, ShellSyntax::Fish);
        assert!(fish.contains("alias claude 'edgee launch claude'"));
        assert!(!fish.contains("$HOME/.edgee/bin"));
    }

    #[test]
    fn appends_block_after_existing_content() {
        let block = render_block(&[OPENCODE_ALIAS], ShellSyntax::Fish);
        let updated = replace_or_append_block("set -gx EDITOR vim\n", &block).unwrap();
        assert_eq!(updated, format!("set -gx EDITOR vim\n\n{block}"));
    }

    #[test]
    fn replaces_existing_block() {
        let old = format!("{MARKER_START}\nalias claude='old value'\n{MARKER_END}\n");
        let new_block = render_path_export_block(ShellSyntax::Posix);
        let updated =
            replace_or_append_block(&format!("export PATH=x\n\n{old}"), &new_block).unwrap();
        assert!(!updated.contains("old value"));
        assert!(updated.contains("$HOME/.edgee/bin"));
    }

    #[test]
    fn strip_managed_block_removes_block_keeps_rest() {
        let block = render_path_export_block(ShellSyntax::Posix);
        let updated = strip_managed_block(&format!("export PATH=x\n\n{block}")).unwrap();
        assert_eq!(updated, "export PATH=x\n\n");
        assert!(!updated.contains(MARKER_START));
    }

    // Non-unix alias-removal path: keeps other aliases, drops the last.
    #[test]
    fn removing_one_agent_preserves_other_aliases() {
        let initial = render_alias_block(&ALL_ALIASES, ShellSyntax::Posix);
        let updated =
            subtract_aliases_from_text(&initial, &codex_only(), ShellSyntax::Posix).unwrap();
        assert!(updated.contains("alias claude='edgee launch claude'"));
        assert!(updated.contains("alias opencode='edgee launch opencode'"));
        assert!(!updated.contains("alias codex='edgee launch codex'"));
    }

    #[test]
    fn removing_last_agent_drops_entire_block() {
        let initial = render_alias_block(&claude_only(), ShellSyntax::Posix);
        let updated =
            subtract_aliases_from_text(&initial, &claude_only(), ShellSyntax::Posix).unwrap();
        assert!(!updated.contains(MARKER_START));
        assert!(!updated.contains(MARKER_END));
    }

    #[test]
    fn removing_from_absent_block_is_noop() {
        let updated =
            subtract_aliases_from_text("export PATH=x\n", &claude_only(), ShellSyntax::Posix)
                .unwrap();
        assert_eq!(updated, "export PATH=x\n");
    }

    #[test]
    fn errors_on_partial_block() {
        let block = render_block(&ALL_ALIASES, ShellSyntax::Posix);
        let err = replace_or_append_block(MARKER_START, &block).unwrap_err();
        assert!(err.to_string().contains("partial Edgee alias block"));
    }

    #[test]
    fn shim_script_strips_shim_dir_from_path_then_exec_launch() {
        let body = render_shim_script("edgee launch claude");
        assert!(body.starts_with("#!/usr/bin/env bash\n"));
        assert!(body.contains("sed -e \"s|:$HOME/.edgee/bin:|:|g\""));
        assert!(body.contains("exec edgee launch claude \"$@\""));
    }

    #[test]
    fn block_contains_alias_detects_posix_and_fish() {
        let posix = "alias claude='edgee launch claude'\n";
        let fish = "alias claude 'edgee launch claude'\n";
        assert!(block_contains_alias(posix, "claude"));
        assert!(block_contains_alias(fish, "claude"));
        assert!(!block_contains_alias(posix, "codex"));
    }

    #[cfg(unix)]
    #[test]
    fn write_shims_creates_executable_file_with_expected_body() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        write_shims(&dir, &claude_only()).unwrap();

        let shim = dir.join("claude");
        let body = std::fs::read_to_string(&shim).unwrap();
        assert!(body.contains("exec edgee launch claude \"$@\""));

        let mode = std::fs::metadata(&shim).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn remove_shims_deletes_only_targeted_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        write_shims(&dir, &ALL_ALIASES).unwrap();
        assert!(dir.join("claude").exists());
        assert!(dir.join("codebuddy").exists());
        assert!(dir.join("codex").exists());
        assert!(dir.join("opencode").exists());

        remove_shims(&dir, &codex_only()).unwrap();
        assert!(dir.join("claude").exists());
        assert!(dir.join("codebuddy").exists());
        assert!(!dir.join("codex").exists());
        assert!(dir.join("opencode").exists());
    }

    #[cfg(unix)]
    #[test]
    fn any_shims_remain_tracks_shim_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        write_shims(&dir, &claude_only()).unwrap();
        assert!(any_shims_remain(&dir));
        remove_shims(&dir, &claude_only()).unwrap();
        assert!(!any_shims_remain(&dir));
    }
}
