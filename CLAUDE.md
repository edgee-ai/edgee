# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

Edgee is an **open-source AI Gateway** written in Rust. It sits between coding agents (Claude Code, Codex, OpenCode — Cursor and OpenClaw coming soon) or any llm client and LLM providers (Anthropic, OpenAI) and compresses token-heavy traffic on the fly. A hosted / edge version of the same gateway is available at [`www.edgee.ai`](https://www.edgee.ai); **this repository is the OSS core** you can self-host.

The distinguishing feature is the compression engine. Today it ships a single technique — **tool-results compression** — but the architecture is explicitly designed to host **multiple composable techniques** that a developer selects and combines per request. When extending compression, add a new technique alongside the existing ones rather than threading a new code path through the provider dispatch layer.

**Verify correct installation:**
```bash
edgee --version  # Should show "edgee 0.2.2" (or newer)
edgee stats      # Should show token savings stats (NOT "command not found")
```

If `edgee stats` fails, you have the wrong package installed.

## CLI surface

Entry point: `crates/cli/src/main.rs`. Subcommands declared in `crates/cli/src/commands/mod.rs`:

- `edgee launch {claude|codex|opencode}` — launches the agent with `ANTHROPIC_BASE_URL` and custom headers pointing at the local gateway. Implementation per agent under `crates/cli/src/commands/launch/`.
- `edgee auth {login|status|list|switch}` — OAuth-style flow against the Edgee console. See `crates/cli/src/api.rs` and `crates/cli/src/commands/auth/`.
- `edgee stats` (visible alias `report`) — prints session token counts and compression savings.
- `edgee alias` — installs shell aliases for quick access.
- `edgee reset` — clears credentials.
- `edgee self-update` — compiled in only under the `self-update` feature.

Global flag: `-p/--profile` overrides the active profile.

## Development Commands

### Build & Run
```bash
cargo build                   # raw
cargo build --release         # release build (optimized)
cargo run -- <command>        # run directly
cargo install --path .        # install locally
```

### Testing
```bash
cargo test                    # all tests
cargo test <test_name>        # specific test
cargo test <module_name>::    # module tests
cargo test -- --nocapture     # with stdout
```

### Linting & Quality
```bash
cargo check                   # check without building
cargo fmt                     # format code
cargo clippy --all-targets    # all clippy lints
```

### Pre-commit Gate
```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
```

### Package Building
```bash
cargo deb                     # DEB package (needs cargo-deb)
cargo generate-rpm            # RPM package (needs cargo-generate-rpm, after release build)
```

## Workspace layout

Cargo workspace (resolver 3), members under `crates/`:

| Crate | Path | Purpose |
|---|---|---|
| `edgee-cli` | `crates/cli` | The `edgee` binary. Launches coding agents, manages auth / profiles / session stats. |
| `edgee-ai-gateway-core` | `crates/gateway-core` | Canonical request/response types, `Provider` trait, passthrough services, `ProviderDispatchService`. No hard tokio/reqwest dependency — runs on WASM/Fastly too. |
| `edgee-compressor` | `crates/compressor` | Pure compression library. Per-tool and per-bash-command strategies. No I/O. |
| `edgee-compression-layer` | `crates/compression-layer` | Tower `Layer` / `Service` that applies `edgee-compressor` to in-flight requests. |


## Architecture — request flow

The gateway is a Tower `Service` chain:

```text
CompletionRequest
      │
      v
┌──────────────────────┐
│  [User layers]       │  ← Any tower::Layer (compression, logging, …)
└──────┬───────────────┘
       │
       v
┌──────────────────────┐
│  ProviderDispatch    │  ← Service<CompletionRequest>
│  Service             │
└──────────────────────┘
       │
       v
 GatewayResponse
```

The canonical format is OpenAI-Chat-Completions-compatible. `ProviderDispatchService` is intended to translate that into each provider's native format — **today it is a stub** (`crates/gateway-core/src/service.rs:65-71` returns `"not yet implemented"`).

The working path today is **passthrough**: provider-native bodies are forwarded without translation. Two passthrough services:

- `AnthropicPassthroughService` — `POST /v1/messages` (`crates/gateway-core/src/passthrough/anthropic.rs`)
- `OpenAIPassthroughService` — `POST /v1/responses` (`crates/gateway-core/src/passthrough/openai.rs`)

Both strip hop-by-hop and gateway-internal headers before forwarding. The HTTP backend is abstracted behind `HttpClient` (`crates/gateway-core/src/backend/http.rs`); enable the `tokio` feature to get `ReqwestHttpClient`, or implement `HttpClient` yourself for a different runtime.

## Token compression — current state & roadmap

### Today: tool-results compression

Entry point: `compress_tool_output(tool_name, arguments, output)` in `crates/compressor/src/lib.rs:32-35`. It looks up a per-tool compressor and applies it, preserving `<system-reminder>` blocks verbatim via `compress_claude_tool_with_segment_protection` (`crates/compressor/src/util.rs`).

Strategies live under `crates/compressor/src/strategy/`:

- `claude/` — Claude Code tools: `Bash`, `Read`, `Grep`, `Glob`.
- `codex/` — Codex CLI tools.
- `opencode/` — OpenCode tools.
- `bash/` — per-command bash output compressors: `ls`, `find`, `tree`, `grep`, `rg`, `cargo`, `npm`, `tsc`, `eslint`, `pytest`, `diff`, `docker`, `env`, `curl`, `go`, `psql`.

Each compressor implements the `ToolCompressor` trait (`crates/compressor/src/strategy/mod.rs`). Bash sub-compressors implement `BashCompressor`; the `Bash` tool compressor parses out the command and dispatches.

Agent-specific tool naming is selected by `AgentType` in `crates/compression-layer/src/config.rs` — `Claude` (PascalCase tool names), `Codex` (e.g. `shell_command`, `read_file`), or `OpenCode` (lowercase).

The Tower integration lives in `crates/compression-layer/src/{layer.rs,service.rs}`: `CompressionLayer` wraps any `Service<CompletionRequest>`, `CompressionService` intercepts requests, mutates them in-place via `compress.rs`, and forwards to the inner service.


## Build Verification (Mandatory)

**CRITICAL**: After ANY Rust file edits, ALWAYS run the full quality check pipeline before committing:

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
```

**Rules**:
- Never commit code that hasn't passed all 3 checks
- Fix ALL clippy warnings before moving on (zero tolerance)
- If build fails, fix it immediately before continuing to next task

## Working Directory Confirmation

**ALWAYS confirm working directory before starting any work**:

```bash
pwd  # Verify you're in the edgee project root
git branch  # Verify correct branch (main, feature/*, etc.)
```

**Never assume** which project to work in. Always verify before file operations.

## Avoiding Rabbit Holes

**Stay focused on the task**. Do not make excessive operations to verify external APIs, documentation, or edge cases unless explicitly asked.

**Rule**: If verification requires more than 3-4 exploratory commands, STOP and ask the user whether to continue or trust available info.

**Examples of rabbit holes to avoid**:
- Excessive regex pattern testing (trust snapshot tests, don't manually verify 20 edge cases)
- Deep diving into external command documentation (use fixtures, don't research git/cargo internals)
- Over-testing cross-platform behavior (test macOS + Linux, trust CI for Windows)
- Verifying API signatures across multiple crate versions (use docs.rs if needed, don't clone repos)

**When to stop and ask**:
- "Should I research X external API behavior?" → ASK if it requires >3 commands
- "Should I test Y edge case?" → ASK if not mentioned in requirements
- "Should I verify Z across N platforms?" → ASK if N > 2

## Plan Execution Protocol

When user provides a numbered plan (QW1-QW4, Phase 1-5, sprint tasks, etc.):

1. **Execute sequentially**: Follow plan order unless explicitly told otherwise
2. **Commit after each logical step**: One commit per completed phase/task
3. **Never skip or reorder**: If a step is blocked, report it and ask before proceeding
4. **Track progress**: Use task list (TaskCreate/TaskUpdate) for plans with 3+ steps
5. **Validate assumptions**: Before starting, verify all referenced file paths exist and working directory is correct