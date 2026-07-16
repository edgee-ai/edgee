<div align="center">

<p align="center">
  <a href="https://www.edgee.ai">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://cdn.edgee.ai/img/logo-white.svg">
      <img src="https://cdn.edgee.ai/img/logo-black.svg" height="50" alt="Edgee">
    </picture>
  </a>
</p>

**Official Edgee CLI — route coding agents through Edgee and cut token spend.**

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Edgee](https://img.shields.io/badge/discord-edgee-blueviolet.svg?logo=discord)](https://www.edgee.ai/discord)
[![Docs](https://img.shields.io/badge/docs-published-blue)](https://www.edgee.ai/docs/introduction)
[![Twitter](https://img.shields.io/twitter/follow/edgee_ai)](https://twitter.com/edgee_ai)
</div>

---

This repository ships the **`edgee` command-line tool**. Install it, sign in, and launch your coding agent — Claude Code, Codex, Cursor, VS Code + Copilot, and more. Traffic goes through Edgee's hosted gateway, which trims tool outputs on the fly so you send fewer tokens without changing how the agent works.

Install the CLI, sign in, and launch your coding agent — Claude Code, Codex, Cursor, VS Code + Copilot, and more. Traffic goes through Edgee's hosted gateway, which compresses tool outputs on the fly so you send fewer tokens without changing how the agent works.

The production gateway (routing, billing, observability) is operated by Edgee and is **not** built from this repo. Self-hosting is not supported here.

<img width="1997" height="807" alt="ai-gateway-horizontal-light" src="https://github.com/user-attachments/assets/d68ff91d-a488-428e-b99d-f4e1c3ef9242" />

## Why Edgee

- **Drop-in for coding agents:** `edgee launch claude` (or `edgee alias`) points your agent at Edgee — no code changes in your project.
- **Token compression:** Tool outputs (file listings, build logs, test results, …) are trimmed before they reach the model. Same answers, leaner context, lower bill.
- **Session visibility:** `edgee stats` and the Claude Code statusline show token usage and compression savings in real time.
- **Rust-native CLI:** Fast install, small footprint, works on macOS, Linux, and Windows.

---

## Install

**macOS / Linux (curl)**

```bash
curl -fsSL https://edgee.ai/install.sh | bash
```

**Homebrew (macOS)**

```bash
brew install edgee-ai/tap/edgee
```

**Windows (PowerShell)**

```powershell
irm https://edgee.ai/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\edgee\`. You can override the directory with `$env:INSTALL_DIR` before running.

Sign in after install:

```bash
edgee auth login
```

---

## Quickstart

### Launch a coding agent

```bash
# Claude Code
edgee launch claude

# Codex
edgee launch codex

# OpenCode
edgee launch opencode

# CodeBuddy
edgee launch codebuddy

# Crush
edgee launch crush

# Cursor (desktop app)
edgee launch cursor

# GitHub Copilot in VS Code
edgee launch copilot-vscode
```

Any extra flags after the subcommand are forwarded to the underlying agent:

```bash
edgee launch claude --resume abcd          # continue a Claude Code session
edgee launch codex resume                  # resume the last Codex session
edgee launch opencode -c                   # continue the last OpenCode session
edgee launch codebuddy --resume <id>       # resume a CodeBuddy session
```

### Route plain `claude` / desktop apps through Edgee (`edgee alias`)

```bash
edgee alias                 # CLI shims + desktop wrappers (when the app is installed)
edgee alias claude          # one CLI agent
edgee alias cursor          # Cursor.app wrapper (skipped if Cursor is not installed)
edgee alias copilot-vscode  # VS Code wrapper (skipped if VS Code is not installed)
edgee alias remove          # undo
```

This covers two kinds of targets:

1. **CLI agents** (`claude`, `codebuddy`, `codex`, `opencode`, `crush`) — shell aliases plus `~/.edgee/bin` PATH shims (Unix), so interactive and non-interactive shells route through Edgee. Reopen your terminal (or `exec $SHELL -l`) once after install.
2. **Apps** (`cursor`, `copilot-vscode`) — desktop launchers only when the host app is already installed: `~/Applications/* (Edgee).app` on macOS, `.desktop` files on Linux, Start Menu shortcuts on Windows. They run `edgee launch …` under the hood.

### Check savings

```bash
edgee stats
```

---

## Features

### Token compression

Edgee's compression engine analyzes tool outputs and removes noise before they enter the LLM context. Compression runs on the **gateway** (via the gateway `tool-result-trimming` crate); the CLI routes your agent there. From the model's perspective the workflow is unchanged — prompts are just leaner.

### Usage tracking

Real-time visibility into token consumption and compression savings per session (`edgee stats`, Claude Code statusline).

### Agent settings

Configure compression, fallback, and reroute options for your coding-agent key:

```bash
edgee settings
```

---

## Statusline

When you run `edgee launch claude`, Claude Code shows a live statusline with the current session's token usage and compression savings. **No setup required:** the first launch auto-installs the integration into `~/.claude/settings.json`, and subsequent launches reuse it.

### Manage it

```bash
edgee statusline claude install   # run the install manually (idempotent)
edgee statusline claude disable   # turn it off
edgee statusline claude enable    # turn it back on
edgee statusline claude doctor    # diagnose project-level conflicts
edgee statusline claude fix       # overlay Edgee on a conflicting project
```

The install writes two things to `~/.claude/settings.json`:

- `statusLine.command = "edgee statusline render"`: only if you don't already have a statusLine; we never overwrite yours. (Older Edgee versions wrote `edgee statusline` without the explicit subcommand; that form now prints help, and is auto-migrated to `edgee statusline render` on next launch.)
- A `SessionStart` hook running `edgee statusline claude doctor --warn-only`, which prints a one-line warning when you open a project that shadows Edgee.

State is tracked with two empty marker files in `~/.config/edgee/`:

- `statusline-claude.installed`: set after the first auto-install; gates repeats.
- `statusline-claude.disabled`: set by `disable`; tells the launch flow to skip auto-install too.

### Coexistence with project-level statuslines

Claude Code only renders **one** `statusLine`, picked by strict precedence: enterprise > project `.claude/settings.local.json` > project `.claude/settings.json` > user `~/.claude/settings.json`. Any project that defines its own `statusLine` (via project hooks, in-house scripts, or third-party statusline tools) will completely shadow Edgee's user-level statusline.

Edgee ships a generic merge wrapper so the two can coexist:

```bash
# In any project where Edgee is shadowed by a project-level statusLine:
edgee statusline claude doctor   # report: NONE / WRAPPED / SHADOWED
edgee statusline claude fix      # write .claude/settings.local.json with an Edgee overlay
```

`edgee statusline claude fix` writes a `statusLine.command` of the form `edgee statusline wrap '<original>'` into `.claude/settings.local.json` (per-user, gitignored). The shared `.claude/settings.json` is **never** touched. Each Claude Code refresh then runs Edgee's renderer and the wrapped command in parallel and merges their outputs into a single line.

**Precedence guarantee:** Edgee's segment is always emitted and is never the one that gets truncated. The wrapped command's output is truncated with `…` to fit the remaining `COLUMNS` budget, ANSI- and Unicode-aware (CJK and emoji are correctly counted as wide). If the wrapped command times out, errors, or returns nothing, only Edgee's segment renders.

The `SessionStart` hook installed by `edgee statusline claude install` (or by the auto-install on first launch) prints a single warning line whenever the current project's statusLine shadows Edgee, and stays silent otherwise.

### Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `EDGEE_STATUSLINE_TIMEOUT_MS` | `2000` | Total timeout for the wrap merge (Edgee + wrapped command). |
| `EDGEE_STATUSLINE_SEPARATOR` | `" │ "` | String inserted between Edgee's segment and the wrapped output. |
| `EDGEE_STATUSLINE_POSITION` | `left` | Either `left` (Edgee on the left, wrapped truncated on the right; recommended) or `right`. |
| `EDGEE_STATUSLINE_PASS_STDERR` | unset | Set to `1` to forward the wrapped command's stderr to the terminal (off by default). |
| `EDGEE_STATUSLINE_MIN_WRAPPED_WIDTH` | `10` | When the wrapped budget falls below this many cells, drop the wrapped output rather than show a stub. |
| `EDGEE_NO_AUTO_OVERLAY` | unset | Set to `1` to make `edgee statusline claude fix` print the suggested overlay instead of writing it (for users who manage `.claude` via dotfiles). |
| `EDGEE_SILENCE_CONFLICT_WARNING` | unset | Set to `1` to silence the `SessionStart` warning. Per-user via shell env, or per-project via `.claude/settings.local.json`'s `env` block. |

---

## Supported agents

| Tool | Setup command | Status |
|---|---|---|
| Claude Code (CLI) | `edgee launch claude` | ✅ Supported |
| Codex (CLI) | `edgee launch codex` | ✅ Supported |
| OpenCode (CLI) | `edgee launch opencode` | ✅ Supported |
| CodeBuddy (CLI) | `edgee launch codebuddy` | ✅ Supported |
| Crush (CLI) | `edgee launch crush` | ✅ Supported |
| Cursor (app) | `edgee launch cursor` | ✅ Supported |
| GitHub Copilot in VS Code | `edgee launch copilot-vscode` | ✅ Supported |

Launch target naming rules (CLI vs apps, suffixes, provider keys) are documented in [`crates/cli/src/commands/launch/README.md`](crates/cli/src/commands/launch/README.md).

---

## Acknowledgments

The token trimming engine in the [Edgee gateway](https://github.com/edgee-ai/gateway) (`tool-result-trimming` crate) is derived from [RTK](https://github.com/rtk-ai/rtk), created by [Patrick Szymkowiak](https://github.com/pszymkowiak) and contributors at rtk-ai Labs. RTK pioneered local tool-output compression for AI coding assistants; we extended that work for gateway-side compression at scale.

RTK is licensed under the Apache License 2.0. All derived files retain the original copyright notice and are individually marked with a modification history. See [`LICENSE-APACHE`](./LICENSE-APACHE) and [`NOTICE`](./NOTICE) for full details.

If you're looking for a local-first compression tool, [check out RTK directly](https://github.com/rtk-ai/rtk) — it's excellent for individual developer workflows.

---

## Repository layout

```
crates/
  cli/                 # edgee binary (auth, launch, stats, alias, relay for GUI apps)
doc/
  architecture.md      # how the CLI relates to the hosted gateway
```

| Crate | Purpose |
|---|---|
| `edgee-cli` | `edgee` binary — launch agents, auth, stats, aliases |

See [`doc/architecture.md`](doc/architecture.md) for how this repo relates to the hosted gateway.

---

## Contributing

Edgee is Apache 2.0 licensed and we genuinely want your contributions.

```bash
git clone https://github.com/edgee-ai/edgee
cd edgee
cargo build
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full guide. For bigger changes, open an issue first so we can align before you build.

---

## Community

- [Discord](https://www.edgee.ai/discord): fastest way to get help
- [GitHub Issues](https://github.com/edgee-ai/edgee/issues): bugs and feature requests
- [Twitter / X](https://twitter.com/edgee_ai): updates and releases
