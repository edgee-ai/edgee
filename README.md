<div align="center">

<p align="center">
  <a href="https://www.edgee.ai">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://cdn.edgee.ai/img/logo-white.svg">
      <img src="https://cdn.edgee.ai/img/logo-black.svg" height="50" alt="Edgee">
    </picture>
  </a>
</p>

**Open-source LLM gateway written in Rust.**
Route, observe, and compress your AI traffic.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Edgee](https://img.shields.io/badge/discord-edgee-blueviolet.svg?logo=discord)](https://www.edgee.ai/discord)
[![Docs](https://img.shields.io/badge/docs-published-blue)](https://www.edgee.ai/docs/introduction)
[![Twitter](https://img.shields.io/twitter/follow/edgee_ai)](https://twitter.com/edgee_ai)
</div>

---

Edgee is a lightweight LLM gateway that sits between your application and AI providers. It gives you a single control point for routing, observability, and cost optimization, without changing your existing code.

Think of it as an open-source alternative to LiteLLM or OpenRouter, written in Rust for speed and low resource usage, with a built-in token compression engine that reduces your AI costs automatically.

<img width="1997" height="807" alt="ai-gateway-horizontal-light" src="https://github.com/user-attachments/assets/09829f8f-cbf3-4afe-8947-bd4cd421667f" />

## Why Edgee

- **One gateway, any provider** — Unified API for Anthropic, OpenAI, and other LLM providers. Switch models without touching your app code.
- **Token compression** — Edgee analyzes request context and strips redundancy before it reaches the model. Same output, fewer tokens, lower bill.
- **Real-time observability** — See exactly how many tokens you're sending, how many you're saving, and what it costs.
- **Rust-native** — Fast startup, minimal memory footprint, no runtime dependencies. Runs anywhere Docker runs.

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

---

## Quickstart

### Use with AI coding assistants

Edgee can wrap your coding assistant and compress traffic automatically:

```bash
# Claude Code
edgee launch claude

# Codex
edgee launch codex

# Opencode
edgee launch opencode
```

Any extra flags you pass after the subcommand are forwarded straight to the underlying agent. For example, to resume the most recent session:

```bash
edgee launch claude --resume abcd # continue the last Claude Code session
edgee launch codex resume         # resume the last Codex session
edgee launch opencode -c          # continue the last OpenCode session
```

### Use as a standalone gateway

Point any OpenAI-compatible client at Edgee:

```bash
# Start the gateway
edgee serve

# Your app talks to Edgee instead of the provider directly
export OPENAI_BASE_URL=http://localhost:1207/v1
```

---

## Features

### Token compression

Edgee's compression engine analyzes tool outputs (file listings, git logs, build output, test results) and removes noise before they enter the LLM context. The compression is lossless from the model's perspective — responses are identical, but prompts are leaner.

### Multi-provider routing

Route requests across Anthropic, OpenAI, and other providers through a single endpoint. Switch models, load-balance, or failover without code changes.

### Usage tracking

Real-time visibility into token consumption, compression savings, and cost per request.

---

## Coexistence with other Claude Code statuslines

Claude Code only renders **one** `statusLine`, picked by strict precedence: enterprise > project `.claude/settings.local.json` > project `.claude/settings.json` > user `~/.claude/settings.json`. Any project that defines its own `statusLine` (via project hooks, in-house scripts, or third-party statusline tools) will completely shadow Edgee's user-level statusline.

Edgee ships a generic merge wrapper so the two can coexist:

```bash
# In any project where Edgee is shadowed by a project-level statusLine:
edgee doctor   # report: NONE / WRAPPED / SHADOWED
edgee fix      # write .claude/settings.local.json with an Edgee overlay
```

`edgee fix` writes a `statusLine.command` of the form `edgee statusline --wrap '<original>'` into `.claude/settings.local.json` (per-user, gitignored). The shared `.claude/settings.json` is **never** touched. Each Claude Code refresh then runs Edgee's renderer and the wrapped command in parallel and merges their outputs into a single line.

**Precedence guarantee:** Edgee's segment is always emitted and is never the one that gets truncated. The wrapped command's output is truncated with `…` to fit the remaining `COLUMNS` budget, ANSI- and Unicode-aware (CJK and emoji are correctly counted as wide). If the wrapped command times out, errors, or returns nothing, only Edgee's segment renders.

To enable a one-line warning when a project shadows Edgee, install the user-level integration once:

```bash
edgee install   # writes ~/.claude/settings.json: statusLine + SessionStart hook
```

This adds a `SessionStart` hook running `edgee doctor --warn-only`, which prints a single line on session start whenever the current project's statusLine shadows Edgee, and stays silent otherwise.

### Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `EDGEE_STATUSLINE_TIMEOUT_MS` | `2000` | Total timeout for the wrap merge (Edgee + wrapped command). |
| `EDGEE_STATUSLINE_SEPARATOR` | `" │ "` | String inserted between Edgee's segment and the wrapped output. |
| `EDGEE_STATUSLINE_POSITION` | `left` | Either `left` (Edgee on the left, wrapped truncated on the right — recommended) or `right`. |
| `EDGEE_STATUSLINE_PASS_STDERR` | unset | Set to `1` to forward the wrapped command's stderr to the terminal (off by default). |
| `EDGEE_STATUSLINE_MIN_WRAPPED_WIDTH` | `10` | When the wrapped budget falls below this many cells, drop the wrapped output rather than show a stub. |
| `EDGEE_NO_AUTO_OVERLAY` | unset | Set to `1` to make `edgee fix` print the suggested overlay instead of writing it (for users who manage `.claude` via dotfiles). |
| `EDGEE_SILENCE_CONFLICT_WARNING` | unset | Set to `1` to silence the `SessionStart` warning. Per-user via shell env, or per-project via `.claude/settings.local.json`'s `env` block. |

---

## Supported setups

| Tool | Setup command | Status |
|---|---|---|
| Claude Code | `edgee launch claude` | ✅ Supported |
| Codex | `edgee launch codex` | ✅ Supported |
| Opencode | `edgee launch opencode` | ✅ Supported |
| Cursor | `edgee launch cursor` | 🔜 Coming soon |
| Any OpenAI-compatible client | `edgee serve` | ✅ Supported |

---

## Acknowledgments

The token compression engine in Edgee is derived from [RTK](https://github.com/rtk-ai/rtk), created by [Patrick Szymkowiak](https://github.com/pszymkowiak) and contributors at rtk-ai Labs. RTK pioneered local tool-output compression for AI coding assistants, and we built on their work to bring the same optimizations to a gateway architecture.

RTK is licensed under the Apache License 2.0. All derived files retain the original copyright notice and are individually marked with a modification history. See [`LICENSE-APACHE`](./LICENSE-APACHE) and [`NOTICE`](./NOTICE) for full details.

If you're looking for a local-first compression tool, [check out RTK directly](https://github.com/rtk-ai/rtk), it's excellent for individual developer workflows.

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

- [Discord](https://www.edgee.ai/discord) — fastest way to get help
- [GitHub Issues](https://github.com/edgee-ai/edgee/issues) — bugs and feature requests
- [Twitter / X](https://twitter.com/edgee_ai) — updates and releases