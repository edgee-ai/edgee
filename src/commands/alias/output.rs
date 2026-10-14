use console::style;

use super::Action;

pub(super) fn header(action: Action) {
    println!();
    println!(
        "  {} {}",
        style("edgee").cyan().bold(),
        style("/ aliases").bold()
    );
    let description = match action {
        Action::Install => "Your everyday commands, through Edgee.",
        Action::Remove => "Back to your usual commands and apps.",
    };
    println!("  {}", style(description).dim());
}

pub(super) fn commands(names: &[&str], action: Action, changed_shells: &[&str]) {
    if names.is_empty() {
        return;
    }
    let mut names = names.to_vec();
    names.sort_unstable();
    println!();
    println!(
        "  {} {}",
        style("Terminal aliases").bold(),
        style(format!("· {}", names.len())).dim()
    );
    let label = match action {
        Action::Install => "Ready",
        Action::Remove => "Removed",
    };
    let width = names.iter().map(|name| name.len()).max().unwrap_or(0);
    for name in names {
        status_row(name, width, label);
    }
    println!();
    if changed_shells.is_empty() {
        println!(
            "    {}",
            style("Shell configuration already up to date.").dim()
        );
    } else {
        let mut shells = changed_shells.to_vec();
        shells.sort_unstable();
        println!(
            "    {}",
            style(format!(
                "Shell configuration updated: {}",
                shells.join(", ")
            ))
            .dim()
        );
    }
}

pub(super) enum ShortcutStatus {
    Installed,
    Updated,
    Removed,
}

pub(super) fn shortcuts_heading() {
    println!();
    println!("  {}", style("App shortcuts").bold());
}

pub(super) fn shortcut(name: &str, width: usize, status: ShortcutStatus) {
    let label = match status {
        ShortcutStatus::Installed => "Created",
        ShortcutStatus::Updated => "Refreshed",
        ShortcutStatus::Removed => "Removed",
    };
    status_row(name, width, label);
}

fn status_row(name: &str, width: usize, label: &str) {
    println!(
        "    {} {name:width$}  {}",
        style("✓").green(),
        style(label).green()
    );
}

pub(super) fn finish(action: Action, commands: usize, shortcuts: usize) {
    println!();
    let title = match action {
        Action::Install if commands > 0 || shortcuts > 0 => "You're all set.",
        Action::Install => "No shortcuts created. Install the app, then try again.",
        Action::Remove if commands > 0 || shortcuts > 0 => "All done.",
        Action::Remove => "Nothing to remove.",
    };
    println!("  {}", style(title).bold());
    if commands > 0 {
        println!("  Open a new terminal to apply the shell changes.");
        if cfg!(unix) {
            println!(
                "  {} {}",
                style("Or run").dim(),
                style("exec $SHELL -l").cyan()
            );
        }
    }
    if matches!(action, Action::Install) && shortcuts > 0 {
        let location = if cfg!(target_os = "macos") {
            "Find your Edgee app shortcuts in ~/Applications."
        } else if cfg!(target_os = "windows") {
            "Find your Edgee app shortcuts in the Start Menu."
        } else {
            "Find your Edgee app shortcuts in your applications menu."
        };
        println!("  {}", style(location).dim());
    }
    println!();
}
