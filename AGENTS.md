## What this repo is

Edgee is an **Agent Gateway** written in Rust. It sits between coding agents (Claude Code, CodeBuddy, Codex, OpenCode, Cursor, GitHub Copilot — more coming) or any llm client and LLM providers (Anthropic, OpenAI) and compresses token-heavy traffic on the fly. **This repository is the OSS CLI Edgee users can use to launch and configure their agents through Edgee**.

**Verify correct installation:**
```bash
edgee --version  # Should show "edgee 0.2.12" (or newer)
edgee stats      # Should show token savings stats (NOT "command not found")
```

If `edgee stats` fails, you have the wrong package installed.

## CLI surface

Entry point: `crates/cli/src/main.rs`. Subcommands declared in `crates/cli/src/commands/mod.rs`:

- `edgee launch {claude|codebuddy|codex|opencode|crush|cursor|copilot}` — launches a coding agent or app through Edgee. CLI agents get gateway env/headers; app targets (`cursor`, `copilot`) use the hidden relay. Naming rules: [`crates/cli/src/commands/launch/README.md`](crates/cli/src/commands/launch/README.md). Implementation per target under `crates/cli/src/commands/launch/`.
- `edgee auth {login|status|list|switch}` — OAuth-style flow against the Edgee console. See `crates/cli/src/api.rs` and `crates/cli/src/commands/auth/`.
- `edgee settings` — configures compression, fallback, and reroute settings for a coding-agent key against the console API.
- `edgee stats` (visible alias `report`) — prints session token counts and compression savings.
- `edgee statusline` — renders/manages the Claude Code statusline integration (see README.md's Statusline section for the install/doctor/fix flow).
- `edgee alias` — installs shell aliases for quick access.
- `edgee reset` — clears credentials.
- `edgee update` — compiled in only under the `self-update` feature.

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

## Code conventions

- **Edition**: the workspace targets Rust edition 2024. `crates/cli` is still pinned to 2021 — check a crate's own `Cargo.toml` before relying on 2024-only syntax there.
- **Dependency versions**: pinned once in the root `Cargo.toml`'s `[workspace.dependencies]`; every crate references them as `dep.workspace = true` (or `{ workspace = true, features = [...] }` to opt into extra features). Add a new dependency to the workspace table first — never as a version pin inside a crate's own `Cargo.toml`.
- **`use` statement grouping**: order imports in blank-line-separated blocks:
  1. `std::...`
  2. external crates (crates.io dependencies)
  3. workspace crates (`edgee_gateway_core`, `edgee_compressor`, `edgee_compression_layer`, `edgee_gateway_http`)
  4. internal (`crate::...`, `super::...`)

  This isn't yet consistently applied across the codebase (e.g. `crates/compression-layer/src/service.rs` mixes an external crate and a workspace crate in one block) — apply the four-block grouping to new and edited code going forward.

## Workspace layout

Cargo workspace (resolver 3), members under `crates/`:

| Crate | Path | Purpose |
|---|---|---|
| `edgee-cli` | `crates/cli` | The `edgee` binary. Launches coding agents, manages auth / profiles / session stats. |
| `edgee-gateway-core` | `crates/gateway-core` | Canonical request/response types, `Provider` trait, passthrough services, `ProviderDispatchService`. No hard tokio/reqwest dependency — runs on WASM/Fastly too. |
| `edgee-gateway-http` | `crates/gateway-http` | `axum-core`-only HTTP boundary. `PassthroughLayer`/`PassthroughService` read the raw request body, strip headers (via the `SKIP_HEADERS` list re-exported from `gateway-core`), and produce a `PassthroughRequest`. Errors serialize in the OpenAI error schema regardless of which provider failed. |
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

The canonical format is OpenAI-Chat-Completions-compatible. `ProviderDispatchService` is intended to translate that into each provider's native format — **today it is a stub** (`crates/gateway-core/src/service.rs`, `Service::call` unconditionally returns an `Error::HttpClient("ProviderDispatchService: not yet implemented")`).

The working path today is **passthrough**: provider-native bodies are forwarded without translation. Two passthrough services:

- `AnthropicPassthroughService` — `POST /v1/messages` (`crates/gateway-core/src/passthrough/anthropic.rs`)
- `OpenAIPassthroughService` — `POST /v1/responses` (`crates/gateway-core/src/passthrough/openai.rs`)

Neither service filters headers itself — both trust `req.headers` as already-clean and forward them as-is. Hop-by-hop and gateway-internal header stripping happens once, upstream, in `gateway-http`'s `PassthroughService` (using the `SKIP_HEADERS` list defined in `crates/gateway-core/src/passthrough/mod.rs`). The intended integration pattern for anyone embedding these crates is to stack `PassthroughLayer` (gateway-http) → `CompressionLayer` (compression-layer) → `Anthropic`/`OpenAIPassthroughService` (gateway-core) via `tower::ServiceBuilder`, one stack per route (`/v1/messages`, `/v1/responses`). The HTTP backend is abstracted behind `HttpClient` (`crates/gateway-core/src/backend/http.rs`); enable the `tokio` feature to get `ReqwestHttpClient`, or implement `HttpClient` yourself for a different runtime.

## Token compression — current state & roadmap

### Today: tool-results compression

Entry point: `compress_tool_output(tool_name, arguments, output)` in `crates/compressor/src/lib.rs`. It looks up a per-tool compressor and applies it, preserving `<system-reminder>` blocks verbatim via `compress_claude_tool_with_segment_protection` (`crates/compressor/src/util.rs`).

Strategies live under `crates/compressor/src/strategy/`:

- `claude/` — Claude Code tools: `Bash`, `Read`, `Grep`, `Glob`.
- `codex/` — Codex CLI tools.
- `opencode/` — OpenCode tools.
- `bash/` — per-command bash output compressors, further grouped by category subdirectory (`fs/`, `rust/`, `js/`, `python/`, `go/`, `sys/`, `vcs/`), each with its own dispatch `mod.rs`.

Each compressor implements the `ToolCompressor` trait (`crates/compressor/src/strategy/mod.rs`). Bash sub-compressors implement `BashCompressor`; the `Bash` tool compressor parses out the command and dispatches.

Agent-specific tool naming is selected by `AgentType` in `crates/compression-layer/src/config.rs` — `Claude` (PascalCase tool names), `Codex` (e.g. `shell_command`, `read_file`), or `OpenCode` (lowercase).

The Tower integration lives in `crates/compression-layer/src/{layer.rs,service.rs}`: `CompressionLayer` wraps any `Service<CompletionRequest>`, `CompressionService` intercepts requests, mutates them in-place via the `compress/` module (`dispatch.rs` for tool-result compression, `passthrough.rs` for the passthrough body path), and forwards to the inner service.

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
