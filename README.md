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

# Oh My Pi
edgee launch omp

# DeepSeek Harness (dsh)
edgee launch deepseek                 # Web UI
edgee launch deepseek headless "run tests"

# Kimi Code
edgee launch kimi

# Kilo Code
edgee launch kilo

# Cursor (desktop app)
edgee launch cursor

# GitHub Copilot in VS Code
edgee launch copilot-vscode
edgee launch intellij       # GitHub Copilot in IntelliJ IDEA
edgee launch phpstorm       # GitHub Copilot in PHPStorm

# GitHub Copilot CLI
edgee launch copilot-cli
edgee launch copilot-desktop  # GitHub Copilot app (macOS)

# Claude Desktop (app)
edgee launch claude-desktop

# ChatGPT desktop app
edgee launch codex-desktop
```

> **Cursor.** Fully quit Cursor before launching. Edgee configures Cursor's
> OpenAI-compatible BYOK provider in `state.vscdb`, registers gateway models,
> then opens the app. To use your Cursor subscription instead, fully quit Cursor
> and run `edgee relay cursor`; this restores your previous provider settings and
> starts the Plan relay.

> **ChatGPT desktop app.** Quit any running instance first — the app only picks up
> the Edgee settings on a fresh start, and an already-running one keeps talking
> straight to OpenAI (the command tells you when this happens).
>
> Edgee writes its provider into `~/.codex/config.toml`, launches the app, and
> **keeps the file patched until you quit the app**, then restores it. The app
> re-reads that config every time you open a tab, so **leave the terminal open** —
> closing it (or Ctrl-C) restores the config, and tabs opened after that talk
> straight to OpenAI. While it runs, a bare `codex` on the CLI also routes through
> Edgee but is billed to your desktop key; use `edgee launch codex` to meter it as
> the CLI. Your `auth.json` is never read or modified.

> **Codex model picker.** `edgee launch codex` and `edgee launch codex-desktop` give
> Codex your org's Edgee model catalog, so models routed through Edgee (Claude,
> Kimi, GLM…) show up in the picker with their real context window and reasoning
> levels. Signed in to ChatGPT, your plan's OpenAI models stay listed first with
> OpenAI's own metadata.

> **GitHub Copilot CLI, one-time trust (macOS).** Unlike the other CLI agents, this one relays rather than
> injects env vars: Copilot CLI's only BYOK lever (`COPILOT_PROVIDER_BASE_URL`)
> replaces GitHub's own model routing and skips GitHub auth entirely, so it can't
> meter your actual paid Copilot seat. `edgee launch copilot-cli` instead spawns
> `copilot` behind the same relay `copilot-vscode` uses, keeping your real GitHub
> OAuth session and rerouting its billed traffic through the gateway. Its native
> TLS client requires the dedicated Copilot CA in the macOS system keychain, so
> the first launch asks for your admin password once.

> **Claude Desktop, one-time trust (macOS).** Claude Desktop (Chromium) checks TLS
> against the macOS **system** keychain, so the first `edgee launch claude-desktop`
> asks for your admin password once to trust a dedicated Edgee CA. That CA is
> name-constrained to `anthropic.com` (SSL policy only), so it can vouch for nothing
> else — but it persists after uninstalling Edgee. Remove it anytime with:
>
> ```bash
> edgee relay claude-desktop --untrust
> ```

> **Copilot surfaces, one-time trust (macOS).** Copilot CLI's native TLS client,
> the desktop app's bundled CLI, and VS Code's Electron network stack check the
> Copilot inference host against the macOS **system** keychain. The first launch
> through any of these surfaces asks for your admin password once to trust a shared,
> name-constrained Edgee Copilot CA. Remove it anytime with any matching target:
>
> ```bash
> edgee relay copilot-cli --untrust
> ```

Any extra flags after the subcommand are forwarded to the underlying agent:

```bash
edgee launch claude --resume abcd          # continue a Claude Code session
edgee launch codex resume                  # resume the last Codex session
edgee launch opencode -c                   # continue the last OpenCode session
edgee launch codebuddy --resume <id>       # resume a CodeBuddy session
```

### Reroute a session at launch

Put Edgee reroute options **before the agent name**:

```bash
edgee launch --reroute openai/gpt-5 claude
edgee launch --reroute openai/gpt-5 --reroute-effort low --reroute-duration 120 codex
```

Edgee validates the target against your organization's model catalog, including
any provider and effort selection, then creates the session reroute before starting
the agent. Model aliases are accepted. If the API rejects
the reroute or cannot be reached, the agent does not start. This requires an API
deployment that accepts reroutes before session traffic exists.

By default, all source models reroute for 60 minutes. Use `--reroute-from MODEL`
to restrict the source model, `--reroute-provider PROVIDER` to pin a catalog
provider, and `--reroute-duration MINUTES` to set a lifetime from 1 to 1440 minutes.
After expiry, normal routing resumes, even if the agent is still running.
The override is scoped to this launch's session and agent API key. Desktop targets
only cover traffic they already route through Edgee; Cursor's current launcher
does not support session reroutes.

### Route plain `claude` / desktop apps through Edgee (`edgee alias`)

```bash
edgee alias                 # CLI shims + desktop wrappers (when the app is installed)
edgee alias claude          # one CLI agent
edgee alias copilot-cli     # GitHub Copilot CLI (installs a `copilot` shim)
edgee alias cursor          # Cursor.app wrapper (skipped if Cursor is not installed)
edgee alias copilot-vscode  # VS Code wrapper (skipped if VS Code is not installed)
edgee alias intellij        # IntelliJ IDEA Copilot wrapper (requires the IDE installed)
edgee alias phpstorm        # PHPStorm Copilot wrapper (requires the IDE installed)
edgee alias copilot-desktop # GitHub Copilot app wrapper (macOS)
edgee alias claude-desktop  # Claude Desktop wrapper (skipped if Claude Desktop is not installed)
edgee alias remove          # undo
```

This covers two kinds of targets:

1. **CLI agents** (`claude`, `codebuddy`, `codex`, `opencode`, `crush`, `pi`, `omp`, `deepseek`, `kimi`, `kilo`) — shell aliases plus `~/.edgee/bin` PATH shims (Unix), so interactive and non-interactive shells route through Edgee. Reopen your terminal (or `exec $SHELL -l`) once after install.
2. **Apps** (`cursor`, `copilot-vscode`, `copilot-desktop`, `claude-desktop`) — desktop launchers only when the host app is already installed: `~/Applications/* (Edgee).app` on macOS, `.desktop` files on Linux, Start Menu shortcuts on Windows. They run `edgee launch …` under the hood.

### Check savings

```bash
edgee stats
```

---

## Features

### Routing: fallbacks and reroutes

Configure a coding-agent key in the [Edgee console](https://www.edgee.ai) to use another model
as a **fallback** (when the usual model errors or is rate-limited) or a **reroute** (every request).

Routing runs on the gateway, so it applies to every request from that key regardless of which
machine or agent surface it came from.

### Token compression

Edgee's compression engine analyses tool outputs and removes noise before they enter the LLM
context. Compression runs on the **gateway**, the CLI routes your agent there. 
From the model's perspective the workflow is unchanged, the prompts are just leaner.

Configure compression for a coding-agent key from the CLI:

```bash
edgee settings           # pick an agent
edgee settings claude    # go straight to one agent's key
```

### Usage tracking

Real-time visibility into token consumption and compression savings per session, via `edgee stats`
and the Claude Code mod. Team-wide cost and usage reporting lives in the
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

## Claude Code mod

`edgee launch claude` loads Edgee's bundled mod on Claude Code 2.1.287 or newer
when Edgee MCP is enabled. Its pane shows observed requests, served models, and
token counts, and lets you reroute the session. Sessions start with a compact line
above the prompt by default. Use `/edgee panel` to expand or `/edgee minimize` to
collapse; subsequent sessions remember your choice.

The mod replaces Edgee's old statusline integration. Launching Claude removes old
Edgee statusline settings and hooks from user settings and project-local settings,
restoring commands wrapped by Edgee. Shared project settings are not modified;
remove legacy Edgee entries there manually if warned.

Mod totals cover observed turn requests only; they do not include all gateway
traffic, gateway cost, or fallback alerts. Use the [Edgee console](https://www.edgee.ai)
for gateway reporting. When mods or MCP are disabled, or Claude Code is older,
there is no Edgee inline display; an older Claude Code gets a hint to run `claude update`. Custom statuslines remain supported by Claude Code.

See [mod commands and limits](mods/edgee/README.md). Set `EDGEE_MODS_DISABLED=1`
to disable the mod, or `EDGEE_NO_UPDATE_CHECK=1` to skip CLI update checks.

---

## GitHub Copilot CLI statusline

`edgee launch copilot-cli` shows the session in Copilot CLI's statusline: the last model served,
requests, tokens, cache hit, cost and what Edgee saved, plus a warning when a request fell back to
another model.

```
◆ Edgee  claude-sonnet-4.6 · 142 req · 4.1M↑ 197k↓ · 92% cache · $1.24 · $0.31 saved
```

Each launch makes sure `~/.copilot/settings.json` (or `$COPILOT_HOME/settings.json`) has
`statusLine.command = "\"${EDGEE_BIN:-edgee}\" statusline"`. `EDGEE_BIN` is the `edgee` that launched the
session, so it works whichever `edgee` is on Copilot's `PATH`. A statusLine of your own is never
replaced. Outside `edgee launch` the segment renders nothing.

```bash
edgee statusline copilot install          # set it up now (also undoes `uninstall`)
edgee statusline copilot install --wrap   # show Edgee next to your own statusLine
edgee statusline copilot uninstall        # remove it, and stop launch from adding it again
```

`uninstall` leaves a `statusline-copilot.disabled` marker in Edgee's config directory, and gives you
back your own command if `--wrap` had merged the two.

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
| Oh My Pi (CLI) | `edgee launch omp` | ✅ Supported |
| DeepSeek Harness (`dsh`) | `edgee launch deepseek` | ✅ Supported |
| Kimi Code (CLI) | `edgee launch kimi` | ✅ Supported |
| Kilo Code (CLI) | `edgee launch kilo` | ✅ Supported |
| Cursor (app) | `edgee launch cursor` | ✅ Supported |
| GitHub Copilot in VS Code | `edgee launch copilot-vscode` | ✅ Supported |
| GitHub Copilot in IntelliJ IDEA | `edgee launch intellij` | Experimental; live validation pending |
| GitHub Copilot in PHPStorm | `edgee launch phpstorm` | Experimental; live validation pending |
| GitHub Copilot app (macOS, local sessions) | `edgee launch copilot-desktop` | ✅ Supported |
| GitHub Copilot CLI | `edgee launch copilot-cli` | ✅ Supported |
| Claude Desktop (Claude Code) | `edgee launch claude-desktop` | ✅ Supported |
| ChatGPT desktop app (Codex tab) | `edgee launch codex-desktop` | ✅ Supported |

**The two desktop chat apps route their coding surface, not their chat surface.**
Each ships a coding agent that speaks the vendor's public API — which Edgee routes — and
a chat client that speaks the vendor's consumer web backend, which it does not:

- `edgee launch claude-desktop` routes **Claude Code**. The app's own chat talks to
  `claude.ai` directly.
- `edgee launch codex-desktop` routes the **Codex tab**. The ChatGPT tab is the ChatGPT
  web client in the app's bundled Chromium and never reaches the embedded Codex backend
  Edgee configures.

In both cases those chat conversations bill your Claude or ChatGPT plan directly and do
not appear in your Edgee stats.

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
| `src/commands/settings/` | Per-key agent settings |
| `mods/edgee/` | Claude Code model selector and request pane |
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

### GitHub Copilot app

Quit GitHub Copilot completely, then run `edgee launch copilot-desktop` on macOS.
Edgee launches the installed app with its connection configured and keeps that connection
alive until the app quits. Ctrl-C closes the launched app before stopping the connection.
`edgee alias copilot-desktop` installs a **GitHub Copilot (Edgee)** desktop wrapper.

Local sessions retain your GitHub Copilot subscription and are metered under **Copilot**,
sharing `edgee settings copilot` with the CLI and VS Code. Remote/cloud sessions are not
covered. Edgee plugin delivery and Windows/Linux desktop launch are not yet supported.
Install the app from https://github.com/features/ai/github-app into `/Applications` or
`~/Applications`. Verified with app 1.1.20, including a tool call and follow-up response.
