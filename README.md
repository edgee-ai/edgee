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
[![Edgee](https://img.shields.io/badge/discord-edgee-blueviolet.svg?logo=discord)](https://www.edgee.ai/discord)
[![Docs](https://img.shields.io/badge/docs-published-blue)](https://www.edgee.ai/docs/introduction)
[![Twitter](https://img.shields.io/twitter/follow/edgee_ai)](https://twitter.com/edgee_ai)
</div>

---

AI coding assistants are incredible. They're also expensive. Every prompt you send to Claude Code or Codex carries context, your files, your history, your instructions, and your token consumption is crazy.

Edgee sits between your coding agent and the LLM APIs and compresses that context before it reaches the model. Same output. Fewer tokens. Lower bill.


<img width="1997" height="807" alt="ai-gateway-horizontal-light" src="https://github.com/user-attachments/assets/09829f8f-cbf3-4afe-8947-bd4cd421667f" />


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

### Launch Claude Code with token compression

```bash
edgee launch claude
```

That's it. Edgee configures itself as a gateway and Claude Code routes through it automatically.

### Launch Codex with token compression

```bash
edgee launch codex
```

---

## What it does

**Token compression** — Edgee analyzes your request context and removes redundancy before sending it upstream. It's lossless from the model's perspective: the response is identical, but the prompt is leaner.

**Usage tracking** — See how many tokens you're sending, how many you're saving, and what it costs — in real time.


---

## Supported setups

| Tool | Setup command | Status |
|---|---|---|
| Claude Code | `edgee launch claude` | ✅ Supported |
| Codex | `edgee launch codex` | ✅ Supported |
| Opencode | `edgee launch opencode` | ✅ Supported |
| Cursor | `edgee launch cursor` | 🔜 Coming soon |


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
