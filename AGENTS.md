# AGENTS.md

Context for coding agents working in this repository. `CLAUDE.md` is a symlink to this file.

## What Edgee is

Edgee is an **Agent Gateway**, built in Rust by Edgee Cloud SAS (Paris) with a US subsidiary,
Edgee Corporation (Delaware).

Edgee sits between AI agents and LLM providers as a transparent gateway. It **intercepts, routes,
compresses, meters, and secures** every LLM request, with no code change on the agent side. It works
with all major coding agents (Claude Code, Codex, Cursor, GitHub Copilot, OpenCode, CodeBuddy,
Crush) and all provider plans, including consumer subscriptions — not just API credits.

**Who it's for:** engineering organisations (typically 150+ developers) whose coding-agent bills are
growing fast and whose leadership lacks visibility and control over token spend. Buyers are VPs of
Engineering, CTOs, and platform teams.

**Core value:** cut token costs by up to 70% while giving engineering leaders full control and
visibility over agent usage.

### Product pillars, in order of strategic importance

1. **Smart routing and budget management** — the moat. The routing engine can redirect requests to
   open-weight models mid-task without quality loss. The upcoming **Strategies** system lets
   customers define routing policies scoped to a person, a squad, or the whole organisation
   (e.g. a $100 weekly Claude budget: at 75% consumption Opus requests reroute to Kimi K3, at 85%
   Sonnet goes to GLM 5.2, at 100% everything routes to DeepSeek).
2. **Team observability** — cost and usage per developer, repo, PR, model, and environment.
3. **Token compression** — input trimming and output brevity, semantically lossless for coding
   tasks. A strong acquisition hook, but expected to become a commodity.

### Positioning guardrails (apply to all user-facing copy)

- The category is **Agent Gateway**. Keep it. Lead with **token cost reduction through routing and
  control** — that's what converts and what wins POCs. Compression is a supporting feature,
  **never the headline**.
- **Never write "Agent Engineering Platform"** (or equivalents) in product copy. It reads as a
  platform for *building* agents — LangChain/CrewAI territory — whereas Edgee *governs* agents. The
  category label only broadens once the desktop app, Strategies, and skills/MCP management have
  shipped and been adopted.
- There is an expansion narrative — enter through cost reduction, become the control plane through
  which every AI agent in the company operates (routing, budgets, observability, skills, MCP,
  governance), the way Datadog expanded from infrastructure monitoring. It is **reserved** for the
  Series A deck, the vision page, and founder talks. It does not belong in the README, the CLI, the
  docs, or any copy you write here.
- Roadmap items may be named as roadmap items (see README) without implying a category change.
- Against **OpenRouter** and **LiteLLM** (the competitors seen in POCs): OpenRouter is a gateway for
  *apps that consume LLMs*; Edgee is specialised for *agents*, wrapping the agent itself with zero
  code change. Differentiators: agent-specialised, token compression, observability per
  developer/repo/PR rather than per API key, native 500-developer onboarding (seats, budgets,
  per-squad policies), and budget-driven Strategies with mid-task rerouting.
- Compliance facts that may be stated: SOC 2 and GDPR compliant, on-premise deployment available,
  BYOK supported.

## What this repo is (and is not)

This repository ships the **`edgee` CLI** — launch agents through Edgee, auth, stats, settings, and
the local relay for GUI apps. It is the **only open-source Edgee repository** (Apache-2.0) and it
will stay open source.

It is **not** the core of Edgee's technology. The production gateway — routing, Strategies,
compression, metering, billing, observability — is proprietary, operated by Edgee, and lives in a
separate private repo. Self-hosting the gateway is not supported from here.

Practical consequence: **compression and routing logic does not live in this repo.** Tool-output
trimming runs gateway-side in the `tool-result-trimming` crate of the gateway repo. Don't go looking
for compression strategies under `src/` — you won't find them, and they don't belong here.

**Verify the right binary is installed** (there is an unrelated package also named `edgee`):

```bash
edgee --version  # Should show "edgee 0.4.0" (or newer — see Cargo.toml)
edgee stats      # Should print session token stats (NOT "command not found")
```

## CLI surface

Entry point: `src/main.rs`. Subcommands declared in `src/commands/mod.rs`:

- `edgee launch {claude|codex|opencode|codebuddy|crush|cursor|copilot-vscode|claude-desktop|codex-desktop}`
  — launches a coding agent or app through Edgee. CLI agents get gateway env/headers; `cursor`,
  `copilot-vscode`, and `claude-desktop` go through the hidden relay; `codex-desktop` patches
  `$CODEX_HOME/config.toml` instead of relaying. Naming rules:
  [`src/commands/launch/README.md`](src/commands/launch/README.md) — **read it before adding a
  target**. Implementation per target under `src/commands/launch/`.
- `edgee auth {login|status|list|switch}` — OAuth-style flow against the Edgee console. See
  `src/api.rs` and `src/commands/auth/`.
- `edgee settings [profile|claude|claude_desktop|codebuddy|codex|codex_desktop|opencode|crush|cursor|copilot]`
  — configures compression, fallback, and reroute settings for a coding-agent key against the
  console API. `edgee settings profile` manages profile-wide (non-agent-specific) settings instead —
  currently the E2EE debug-log encryption passphrase (`src/commands/settings/profile.rs`). The
  provider list uses bare `copilot`, deliberately distinct from launch's `copilot-vscode`, and
  underscored `claude_desktop`/`codex_desktop` for surfaces metered as their own backend agent.
- `edgee stats` (visible alias `report`) — session token counts and compression savings.
- `edgee statusline` — renders/manages the Claude Code statusline integration (README's Statusline
  section has the install/doctor/fix flow).
- `edgee alias` — installs CLI PATH shims/shell aliases and desktop app wrappers (`cursor`,
  `copilot-vscode`) when the host app is installed.
- `edgee relay` — hidden (`hide = true`). Local MITM proxy powering the app launch targets;
  transport is an implementation detail, not public UX.
- `edgee reset` — clears credentials.
- `edgee update` (visible alias `self-update`) — compiled in only under the `self-update` feature
  (on by default).

Root flag: `-p/--profile` overrides the active profile. It must come **before** the subcommand (`edgee -p dev launch claude`).

**Argv rule for `launch`: everything after the target name belongs to the agent.** A flag declared on the target — or a clap `global` arg — wins against the target's `trailing_var_arg` passthrough whenever the user puts it first, silently swallowing an identically-named agent flag. `-p/--profile` used to do this to Claude Code's `-p/--print`, turning `claude -p "my prompt"` into a switch to a profile named "my prompt", so `profile` is deliberately **not** `global`. Two defenses, both load-bearing:

- Don't add flags to a launch target unless the agent has no flag of that name (`--relay` on `claude` is the one such case), and never make a root arg `global`. Pinned by `edgee_flags_do_not_shadow_agent_flags` in `src/commands/launch/mod.rs`.
- The shims `edgee alias` writes end their launch command with `--` (`edgee launch claude -- "$@"`), so the aliased path is immune regardless. clap consumes that first `--` and forwards any later one.

Injected agent flags have a matching hazard: pass any flag the agent declares **variadic** as `--flag=value`, never `--flag value`, or it consumes the user's trailing args — a space-separated `--allowedTools` ate both `claude mcp add …` and the user's prompt. See `mcp_injection_args` in `src/commands/launch/claude.rs`.

## Repo map

```text
src/
  main.rs              # clap entry point, profile resolution, dispatch
  api.rs               # Edgee console API client
  config.rs            # credentials.toml, profiles
  crypto.rs            # X25519 + argon2 for E2EE debug logs
  git.rs               # repo/branch detection for session attribution
  version_check.rs     # update nag (EDGEE_NO_UPDATE_CHECK to silence)
  commands/
    launch/            # one module per target + README.md naming rules
    auth/              # login, status, list, switch
    settings/          # agent.rs (per-key), profile.rs (profile-wide)
    statusline/        # render, wrap, width + claude/ (install, doctor, fix, toggle)
    alias/             # PATH shims + desktop.rs app wrappers
    relay/             # hidden MITM proxy (hudsucker + rcgen)
    util/session_log/  # session tracking
```

## Development commands

```bash
cargo build                   # debug
cargo build --release         # optimized
cargo run -- <command>        # run directly, e.g. cargo run -- launch claude
cargo install --path .        # install locally

cargo test                    # all tests
cargo test <name>             # a single test
cargo test -- --nocapture     # with stdout

cargo check                   # typecheck only
cargo fmt --all               # format
cargo clippy --all-targets    # lint (CI runs with -D warnings)
```

### Pre-commit gate (mandatory)

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
```

### Releases

Releases are cut by `.github/workflows/release.yml` on tag: it builds seven targets
(x86_64/aarch64 × linux-gnu/linux-musl, x86_64/aarch64 macOS, x86_64 Windows MSVC), publishes the
GitHub release, and updates the Homebrew tap formula. There is **no** DEB/RPM packaging in this
repo — don't add `cargo deb` / `cargo generate-rpm` steps without asking.

`.github/workflows/check.yml` runs `cargo check` (unix + windows), `cargo fmt --check`, and
`cargo clippy --all-targets -- -D warnings`.

## Code conventions

- **Edition**: pinned to Rust edition 2021 in `Cargo.toml` — don't rely on edition-2024-only syntax.
- **MSRV**: Rust stable 1.85 or later (see CONTRIBUTING.md).
- **`use` statement grouping**: blank-line-separated blocks, in this order:
  1. `std::...`
  2. external crates (crates.io dependencies)
  3. internal (`crate::...`, `super::...`)

  Apply the three-block grouping to new and edited code going forward.
- **Naming**: modules `snake_case` (`copilot_vscode.rs`), clap subcommand names hyphenated
  (`copilot-vscode`), structs/enums `PascalCase`, constants `SCREAMING_SNAKE_CASE`.
- **Errors**: `anyhow::Result` throughout the CLI; `?` for early return, `.context()` to add
  meaning at boundaries.
- **User-facing output**: `colored` / `console` for styling. Keep messages short and actionable;
  every error a user can hit should say what to do next.

## Build verification (mandatory)

**CRITICAL**: after ANY Rust file edit, run the full quality pipeline before committing:

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
```

**Rules**:
- Never commit code that hasn't passed all 3 checks.
- Fix ALL clippy warnings before moving on (zero tolerance — CI uses `-D warnings`).
- If the build fails, fix it immediately before continuing to the next task.

## Working directory confirmation

**ALWAYS confirm the working directory before starting any work**:

```bash
pwd         # verify you're in the edgee project root
git branch  # verify the correct branch (main, feat/*, fix/*, chore/*)
```

**Never assume** which project to work in. Always verify before file operations.

## Avoiding rabbit holes

**Stay focused on the task.** Don't burn operations verifying external APIs, documentation, or edge
cases unless explicitly asked.

**Rule**: if verification requires more than 3-4 exploratory commands, STOP and ask the user whether
to continue or trust the available information.

**Examples to avoid**:
- Excessive regex pattern testing (trust the tests, don't hand-verify 20 edge cases).
- Deep dives into external command documentation (use fixtures, don't research git/cargo internals).
- Over-testing cross-platform behaviour (test macOS + Linux, trust CI for Windows).
- Verifying API signatures across multiple crate versions (use docs.rs, don't clone repos).

**When to stop and ask**:
- "Should I research X external API behaviour?" → ASK if it takes >3 commands.
- "Should I test Y edge case?" → ASK if it isn't in the requirements.
- "Should I verify Z across N platforms?" → ASK if N > 2.

## Plan execution protocol

When the user provides a numbered plan (QW1-QW4, Phase 1-5, sprint tasks, …):

1. **Execute sequentially**: follow plan order unless explicitly told otherwise.
2. **Commit after each logical step**: one commit per completed phase/task.
3. **Never skip or reorder**: if a step is blocked, report it and ask before proceeding.
4. **Track progress**: use the task list (TaskCreate/TaskUpdate) for plans with 3+ steps.
5. **Validate assumptions**: before starting, verify referenced paths exist and the working
   directory is correct.
