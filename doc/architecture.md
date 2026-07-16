# Architecture

This repository contains the **Edgee CLI** (`edgee-cli`) and the shared
**tool-results compression library** (`edgee-compressor`).

The production AI Gateway that sits between coding agents and LLM providers
(AWS / Fastly / on-prem) lives in a separate repository. That gateway consumes
`edgee-compressor` from crates.io and applies compression on the request path.

## Crate boundaries

| Crate | Crate name | Responsibility |
|---|---|---|
| `crates/cli` | `edgee-cli` | `edgee` binary — launch agents, auth, relay for GUI apps, stats, aliases |
| `crates/compressor` | `edgee-compressor` | Pure compression library (no I/O); published and used by the hosted gateway |

## How the CLI relates to the gateway

```text
coding agent  ──HTTP──►  Edgee gateway (hosted / on-prem)
                              │
                              ├── edgee-compressor (crates.io)
                              └── provider adapters, routing, billing, …
```

`edgee launch <agent>` points the agent at the gateway (`ANTHROPIC_BASE_URL` /
custom headers, or a local MITM relay for apps that cannot be redirected).
Compression of tool results happens **inside the gateway**, via
`edgee-compressor` — not inside the CLI process.

## Token compression (`edgee-compressor`)

Entry point: `compress_tool_output(tool_name, arguments, output)` in
`crates/compressor/src/lib.rs`.

Strategies live under `crates/compressor/src/strategy/`:

- `claude/` — Claude Code tools (`Bash`, `Read`, `Grep`, `Glob`, …)
- `codex/` — Codex CLI tools
- `opencode/` — OpenCode tools
- `bash/` — per-command bash output compressors (`fs/`, `rust/`, `js/`, …)

Each compressor implements `ToolCompressor`. See
[`crates/compressor/README.md`](../crates/compressor/README.md) for details and
how to add a strategy.
