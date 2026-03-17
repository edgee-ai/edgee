<div align="center">

<p align="center">
  <a href="https://www.edgee.ai">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://cdn.edgee.ai/img/logo-white.svg">
      <img src="https://cdn.edgee.ai/img/logo-black.svg" height="50" alt="Edgee">
    </picture>
  </a>
</p>

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Edgee](https://img.shields.io/badge/edgee-open%20source-blueviolet.svg)](https://www.edgee.ai)
[![Edgee](https://img.shields.io/badge/discord-edgee-blueviolet.svg?logo=discord)](https://www.edgee.ai/discord)
[![Docs](https://img.shields.io/badge/docs-published-blue)](https://www.edgee.ai/docs/introduction)
[![Twitter](https://img.shields.io/twitter/follow/edgee_ai)](https://twitter.com/edgee_ai)
</div>

---

AI coding tools are incredible. They're also expensive. Every prompt you send to Claude Code or Codex carries context — your files, your history, your instructions — and you pay for every token, every time.

Edgee sits between your tools and the LLM APIs and compresses that context before it leaves your machine. Same output. Fewer tokens. Lower bill.

```
Claude Code ──► edgee ──► Anthropic API
                 ↑
          token compression
          happens here
```

---

## Install

**macOS / Linux (curl)**

```bash
curl https://install.edgee.ai | sh
```

**Homebrew**

```bash
brew install edgee-ai/tap/edgee
```

Verify your install:

```bash
edgee --version
```

---

## Quickstart

### With Claude Code

```bash
edgee setup claude-code
```

That's it. Edgee configures itself as a local proxy and Claude Code routes through it automatically.

### With Codex

```bash
edgee setup codex
```

### With any OpenAI-compatible tool

```bash
edgee start
```

Then point your tool at `http://localhost:3000` instead of the upstream API. Edgee speaks the OpenAI API spec natively.

---

## What it does

**Token compression** — Edgee analyzes your request context and removes redundancy before sending it upstream. It's lossless from the model's perspective: the response is identical, but the prompt is leaner.

**Local, private** — Everything runs on your machine. Your prompts don't touch Edgee servers. The compression happens locally.

**Drop-in proxy** — Edgee implements the OpenAI API spec. If your tool already talks to an LLM API, it talks to Edgee with zero code changes.

**Usage tracking** — See how many tokens you're sending, how many you're saving, and what it costs — in real time.

```bash
edgee stats
```

---

## Configuration

Edgee stores its config at `~/.edgee/config.toml`. You can also pass a custom path:

```bash
edgee start --config ./edgee.toml
```

**Example config:**

```toml
[proxy]
port = 3000

[compression]
enabled = true
level = "balanced"   # "light" | "balanced" | "aggressive"

[upstream]
provider = "anthropic"  # "anthropic" | "openai" | "custom"
```

---

## Commands

| Command | Description |
|---|---|
| `edgee start` | Start the local gateway |
| `edgee setup <tool>` | Auto-configure a supported tool |
| `edgee stats` | Show token usage and savings |
| `edgee config` | Open config in your editor |
| `edgee stop` | Stop the gateway |
| `edgee --help` | Full command reference |

---

## Supported tools

| Tool | Setup command | Status |
|---|---|---|
| Claude Code | `edgee setup claude-code` | ✅ Supported |
| Codex | `edgee setup codex` | ✅ Supported |
| Opencode | `edgee setup opencode` | ✅ Supported |
| Continue | `edgee setup continue` | 🔜 Coming soon |
| Cursor | `edgee setup cursor` | 🔜 Coming soon |
| Custom (OpenAI-compat) | `edgee start` | ✅ Supported |

---

## Need more?

Edgee is built by the team behind [Edgee Cloud](https://edgee.cloud) — a production-grade AI gateway that runs at the edge, with enterprise token compression, multi-region deployment, team-level cost controls, and full observability.

If you're hitting scale, managing spend across a team, or need SLA-backed infrastructure — [talk to us](https://edgee.cloud/contact).

---

## Contributing

Edgee is Apache 2.0 licensed and we genuinely want your contributions.

```bash
git clone https://github.com/edgee-cloud/edgee
cd edgee
cargo build
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full guide. For bigger changes, open an issue first so we can align before you build.

---

## Community

- [Discord](https://discord.gg/edgee) — fastest way to get help
- [GitHub Issues](https://github.com/edgee-cloud/edgee/issues) — bugs and feature requests
- [Twitter / X](https://twitter.com/edgee_cloud) — updates and releases

---

<div align="center">
  <sub>Built with Rust. Runs at the edge. Open source forever.</sub>
</div>