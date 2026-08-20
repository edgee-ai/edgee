<div align="center">

<p align="center">
  <a href="https://www.edgee.ai">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://cdn.edgee.ai/img/logo-white.svg">
      <img src="https://cdn.edgee.ai/img/logo-black.svg" height="50" alt="Edgee">
    </picture>
  </a>
</p>

**Official Edgee CLI — route your coding agents through Edgee and take control of token spend.**

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Edgee](https://img.shields.io/badge/discord-edgee-blueviolet.svg?logo=discord)](https://www.edgee.ai/discord)
[![Docs](https://img.shields.io/badge/docs-published-blue)](https://www.edgee.ai/docs/introduction)
[![Twitter](https://img.shields.io/twitter/follow/edgee_ai)](https://twitter.com/edgee_ai)
</div>

---

Coding agents are the fastest-growing line item in most engineering budgets — and the least visible
one. **Edgee is an Agent Gateway**: it sits between your agents and the LLM providers, and
intercepts, routes, compresses, meters, and secures every request. No code change in your projects,
no change to how your agents work.

This repository ships the **`edgee` command-line tool**. Install it, sign in, and launch your coding
agent — Claude Code, Codex, Cursor, VS Code + Copilot, and more. Setup takes about five minutes.

The production gateway (routing, compression, metering, billing, observability) is operated by Edgee
and is **not** built from this repo. Self-hosting is not supported here.

<img width="2515" height="1422" alt="illustration" src="https://github.com/user-attachments/assets/4ca2fdc1-ffe2-4f7f-8db1-4e527bb1fc4d" />

## Why Edgee

- **Smart routing and budgets.** Send work to the right model for the job, with fallbacks when your
  primary model errors or rate-limits, and reroutes to cheaper open-weight models when it doesn't
  need to be expensive — mid-task, without losing quality.
- **Visibility your finance team will ask for.** Token cost and usage tracked per developer, repo,
  PR, model, and environment — not per anonymous API key.
- **Token compression.** Tool outputs (file listings, build logs, test results, …) are trimmed
  gateway-side before they reach the model. Same answers, leaner context, smaller bill.
- **Drop-in for coding agents.** `edgee launch claude` (or `edgee alias`) points your agent at
  Edgee. Works with every provider plan, including consumer subscriptions — not just API credits.
- **Rust-native CLI.** Fast install, small footprint, macOS, Linux, and Windows.

Together that's up to **70% off your token bill**, with the control and reporting engineering
leaders need to keep agent adoption growing without the spend growing with it.

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

Installs to `%LOCALAPPDATA%\Programs\edgee\`. You can override the directory with `$env:INSTALL_DIR`
before running.

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

# Pi
edgee launch pi

# Kilo Code
edgee launch kilo

# Cursor (desktop app)
edgee launch cursor

# GitHub Copilot in VS Code
edgee launch copilot-vscode

# Claude Desktop (app)
edgee launch claude-desktop

# ChatGPT desktop app
edgee launch codex-desktop
```

> **ChatGPT desktop app.** Quit any running instance first — the app only picks up
> the Edgee settings on a fresh start, and an already-running one keeps talking
> straight to OpenAI (the command tells you when this happens).
>
> The app reads its config once at startup, so Edgee writes its provider into
> `~/.codex/config.toml`, launches the app, and **reverts the file about ten seconds
> later** — then exits. The app keeps the settings in memory for the rest of the
> session, so you can close the terminal, and your `codex` CLI is unaffected. Your
> `auth.json` is never read or modified.

> **Claude Desktop, one-time trust (macOS).** Claude Desktop (Chromium) checks TLS
> against the macOS **system** keychain, so the first `edgee launch claude-desktop`
> asks for your admin password once to trust a dedicated Edgee CA. That CA is
> name-constrained to `anthropic.com` (SSL policy only), so it can vouch for nothing
> else — but it persists after uninstalling Edgee. Remove it anytime with:
>
> ```bash
> edgee relay claude-desktop --untrust
> ```

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
edgee alias claude-desktop  # Claude Desktop wrapper (skipped if Claude Desktop is not installed)
edgee alias remove          # undo
```

This covers two kinds of targets:

1. **CLI agents** (`claude`, `codebuddy`, `codex`, `opencode`, `crush`, `pi`, `kilo`) — shell aliases plus `~/.edgee/bin` PATH shims (Unix), so interactive and non-interactive shells route through Edgee. Reopen your terminal (or `exec $SHELL -l`) once after install.
2. **Apps** (`cursor`, `copilot-vscode`, `claude-desktop`) — desktop launchers only when the host app is already installed: `~/Applications/* (Edgee).app` on macOS, `.desktop` files on Linux, Start Menu shortcuts on Windows. They run `edgee launch …` under the hood.

### Check savings

```bash
edgee stats
```

---

## Features

### Routing: fallbacks and reroutes

Point a coding-agent key at another model — as a **fallback** (only when the usual model errors or
is rate-limited) or a **reroute** (every request, instead of the usual model):

```bash
edgee settings           # pick an agent, then configure routing and compression
edgee settings claude    # go straight to one agent's key
```

Routing runs on the gateway, so it applies to every request from that key regardless of which
machine or agent surface it came from.

### Token compression

Edgee's compression engine analyses tool outputs and removes noise before they enter the LLM
context. Compression runs on the **gateway**, the CLI routes your agent there. 
From the model's perspective the workflow is unchanged, the prompts are just leaner.

### Usage tracking

Real-time visibility into token consumption and compression savings per session, via `edgee stats`
and the Claude Code statusline. Team-wide cost and usage reporting lives in the
[Edgee console](https://www.edgee.ai).

---

## Roadmap

What's coming next, in the order it matters to us:

- **Strategies** — budget-driven routing policies scoped to a person, a squad, or the whole
  organisation. Set a weekly budget and the rules that kick in as it's consumed: at 75%, reroute
  expensive models to a cheaper open-weight one; at 100%, route everything to your cost floor.
- **Desktop app** — a single Edgee app to launch and monitor your whole local AI stack, CLI agents
  and desktop apps alike, without going through the terminal.
- **Skills and MCP management** — deploy and govern skills and MCP servers across every agent in an
  organisation, from one place.

Together these extend what a gateway can do for an engineering organisation: more of your agents
routed through Edgee, more of your spend under policy, more of your usage visible.

---

## Statusline

When you run `edgee launch claude`, Claude Code shows a live statusline with the current session's
token usage and compression savings. **No setup required:** the first launch auto-installs the
integration into `~/.claude/settings.json`, and subsequent launches reuse it.

### Manage it

```bash
edgee statusline claude install   # run the install manually (idempotent)
edgee statusline claude disable   # turn it off
edgee statusline claude enable    # turn it back on
edgee statusline claude doctor    # diagnose project-level conflicts
edgee statusline claude fix       # overlay Edgee on a conflicting project
```

The install writes two things to `~/.claude/settings.json`:

- `statusLine.command = "edgee statusline render"`: only if you don't already have a statusLine; we
  never overwrite yours. (Older Edgee versions wrote `edgee statusline` without the explicit
  subcommand; that form now prints help, and is auto-migrated to `edgee statusline render` on next
  launch.)
- A `SessionStart` hook running `edgee statusline claude doctor --warn-only`, which prints a
  one-line warning when you open a project that shadows Edgee.

State is tracked with two empty marker files in `~/.config/edgee/`:

- `statusline-claude.installed`: set after the first auto-install; gates repeats.
- `statusline-claude.disabled`: set by `disable`; tells the launch flow to skip auto-install too.

### Coexistence with project-level statuslines

Claude Code only renders **one** `statusLine`, picked by strict precedence: enterprise > project
`.claude/settings.local.json` > project `.claude/settings.json` > user `~/.claude/settings.json`.
Any project that defines its own `statusLine` (via project hooks, in-house scripts, or third-party
statusline tools) will completely shadow Edgee's user-level statusline.

Edgee ships a generic merge wrapper so the two can coexist:

```bash
# In any project where Edgee is shadowed by a project-level statusLine:
edgee statusline claude doctor   # report: NONE / WRAPPED / SHADOWED
edgee statusline claude fix      # write .claude/settings.local.json with an Edgee overlay
```

`edgee statusline claude fix` writes a `statusLine.command` of the form
`edgee statusline wrap '<original>'` into `.claude/settings.local.json` (per-user, gitignored). The
shared `.claude/settings.json` is **never** touched. Each Claude Code refresh then runs Edgee's
renderer and the wrapped command in parallel and merges their outputs into a single line.

**Precedence guarantee:** Edgee's segment is always emitted and is never the one that gets
truncated. The wrapped command's output is truncated with `…` to fit the remaining `COLUMNS` budget,
ANSI- and Unicode-aware (CJK and emoji are correctly counted as wide). If the wrapped command times
out, errors, or returns nothing, only Edgee's segment renders.

The `SessionStart` hook installed by `edgee statusline claude install` (or by the auto-install on
first launch) prints a single warning line whenever the current project's statusLine shadows Edgee,
and stays silent otherwise.

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
| `EDGEE_NO_UPDATE_CHECK` | unset | Set to `1` to skip the background check for a newer CLI release. |

---

## Supported agents

| Tool | Setup command | Status |
|---|---|---|
| Claude Code (CLI) | `edgee launch claude` | ✅ Supported |
| Codex (CLI) | `edgee launch codex` | ✅ Supported |
| OpenCode (CLI) | `edgee launch opencode` | ✅ Supported |
| CodeBuddy (CLI) | `edgee launch codebuddy` | ✅ Supported |
| Crush (CLI) | `edgee launch crush` | ✅ Supported |
| Pi (CLI) | `edgee launch pi` | ✅ Supported |
| Kilo Code (CLI) | `edgee launch kilo` | ✅ Supported |
| Cursor (app) | `edgee launch cursor` | ✅ Supported |
| GitHub Copilot in VS Code | `edgee launch copilot-vscode` | ✅ Supported |
| Claude Desktop (app) | `edgee launch claude-desktop` | ✅ Supported |
| ChatGPT desktop app | `edgee launch codex-desktop` | ✅ Supported |

Launch target naming rules (CLI vs apps, suffixes, provider keys) are documented in
[`src/commands/launch/README.md`](src/commands/launch/README.md).

---

## Repository layout

This repo is a single Rust binary crate (`edgee-cli`, binary name `edgee`).

| Path | Purpose |
|---|---|
| `src/main.rs` | clap entry point: argument parsing, profile resolution, subcommand dispatch |
| `src/api.rs` | Edgee console API client (auth, keys, settings, stats) |
| `src/config.rs` | `credentials.toml` handling and named profiles |
| `src/crypto.rs` | X25519 + Argon2 key derivation for E2EE debug logs |
| `src/git.rs` | Repo/branch detection used to attribute sessions |
| `src/version_check.rs` | Background check for a newer CLI release |
| `src/commands/launch/` | One module per launch target, plus [naming rules](src/commands/launch/README.md) |
| `src/commands/auth/` | `login`, `status`, `list`, `switch` |
| `src/commands/settings/` | Per-key agent settings and profile-wide settings |
| `src/commands/statusline/` | Statusline renderer, wrap/merge logic, Claude integration |
| `src/commands/alias/` | Shell aliases, PATH shims, desktop app wrappers |
| `src/commands/relay/` | Local MITM relay powering the app launch targets |

The gateway itself — routing, compression, metering, billing, observability — is a separate,
Edgee-operated service and is not part of this repository.

---

## Acknowledgments

The token trimming engine in the Edgee gateway is derived from [RTK](https://github.com/rtk-ai/rtk), created by
[Patrick Szymkowiak](https://github.com/pszymkowiak) and contributors at rtk-ai Labs. RTK pioneered
local tool-output compression for AI coding assistants; we extended that work for gateway-side
compression at scale.

RTK is licensed under the Apache License 2.0. All derived files retain the original copyright notice
and are individually marked with a modification history, in the repository where they live.

If you're looking for a local-first compression tool,
[check out RTK directly](https://github.com/rtk-ai/rtk) — it's excellent for individual developer
workflows.

---

## Contributing

This CLI is Apache 2.0 licensed and open source, and we genuinely want your contributions.

```bash
git clone https://github.com/edgee-ai/edgee
cd edgee
cargo build
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full guide and [LICENSE](LICENSE) for the license
text. For bigger changes, open an issue first so we can align before you build.

---

## Community

- [Discord](https://www.edgee.ai/discord): fastest way to get help
- [GitHub Issues](https://github.com/edgee-ai/edgee/issues): bugs and feature requests
- [Twitter / X](https://twitter.com/edgee_ai): updates and releases

---

Edgee is built by Edgee Cloud SAS (Paris) and Edgee Corporation (Delaware). Edgee is SOC 2
and GDPR compliant, supports BYOK, and is available on-premise. Talk to us at
[edgee.ai](https://www.edgee.ai).
