use anyhow::Result;

use super::{util, Command};

/// Fail before login, reroute resolution or onboarding can change account settings.
pub(super) fn check(command: &Command) -> Result<()> {
    let (binary, message) = match command {
        Command::Claude(_) => (
            "claude",
            "Claude Code is not installed. Install it from https://code.claude.com/docs/en/quickstart",
        ),
        Command::Codex(_) => (
            "codex",
            "Codex CLI is not installed. Install it from https://developers.openai.com/codex/cli",
        ),
        Command::CodeBuddy(_) => (
            "codebuddy",
            "CodeBuddy is not installed. Install it from https://cnb.cool/codebuddy/codebuddy-code",
        ),
        Command::OpenCode(_) => (
            "opencode",
            "OpenCode is not installed. Install it from https://opencode.ai",
        ),
        Command::Crush(_) => (
            "crush",
            "Crush is not installed. Install it from https://github.com/charmbracelet/crush",
        ),
        Command::Pi(_) => (
            "pi",
            "Pi is not installed. Install it with `npm install -g @mariozechner/pi-coding-agent`",
        ),
        Command::Omp(_) => (
            "omp",
            "OMP is not installed. Install it from https://github.com/can1357/oh-my-pi",
        ),
        Command::Deepseek(_) => (
            "dsh",
            "DeepSeek Harness is not installed. Install it with `npm install -g @deepseek-ai/dsh`.",
        ),
        Command::Kimi(_) => (
            "kimi",
            "Kimi Code is not installed. Install it with `curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash`",
        ),
        Command::Kilo(_) => (
            "kilo",
            "Kilo Code is not installed. Install it with `npm install -g @kilocode/cli`",
        ),
        Command::CopilotCli(_) => (
            "copilot",
            "GitHub Copilot CLI is not installed. Install it from https://github.com/github/copilot-cli",
        ),
        Command::Cursor(_) => (
            "cursor",
            "Cursor is not on PATH. Install Cursor from https://cursor.com, then run Command Palette → Install 'cursor' command.",
        ),
        Command::CopilotVscode(_) => (
            "code",
            "VS Code is not on PATH. Install it from https://code.visualstudio.com, then run Command Palette → Shell Command: Install 'code' command in PATH.",
        ),
        Command::ClaudeDesktop(_) => {
            return crate::commands::relay::claude_desktop_binary().map(|_| ());
        }
        Command::CodexDesktop(_) => return super::codex_desktop::binary().map(|_| ()),
        Command::CopilotDesktop(_) => return super::copilot_desktop::binary().map(|_| ()),
        Command::Intellij(_) => return super::intellij::binary().map(|_| ()),
        Command::Phpstorm(_) => return super::phpstorm::binary().map(|_| ()),
    };
    anyhow::ensure!(util::find_binary(binary).is_some(), "{message}");
    Ok(())
}
