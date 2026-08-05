## What this repo is


Edgee is an **Agent Gateway** written in Rust. It sits between coding agents (Claude Code, CodeBuddy, Codex, OpenCode, Cursor, GitHub Copilot — more coming) or any llm client and LLM providers (Anthropic, OpenAI) and compresses token-heavy traffic on the fly. **This repository ships the `edgee` CLI (launch agents through Edgee, auth, stats, local relay for GUI apps)**.


**Verify correct installation:**
```bash
edgee --version  # Should show "edgee 0.2.12" (or newer)
edgee stats      # Should show token savings stats (NOT "command not found")
```

If `edgee stats` fails, you have the wrong package installed.

## CLI surface

Entry point: `src/main.rs`. Subcommands declared in `src/commands/mod.rs`:

- `edgee launch {claude|codebuddy|codex|opencode|crush|cursor|copilot}` — launches a coding agent or app through Edgee. CLI agents get gateway env/headers; app targets (`cursor`, `copilot`) use the hidden relay. Naming rules: [`src/commands/launch/README.md`](src/commands/launch/README.md). Implementation per target under `src/commands/launch/`.
- `edgee auth {login|status|list|switch}` — OAuth-style flow against the Edgee console. See `src/api.rs` and `src/commands/auth/`.
- `edgee settings [claude|codebuddy|codex|opencode|crush]` — configures compression, fallback, and reroute settings for a coding-agent key against the console API. `edgee settings profile` manages profile-wide (non-agent-specific) settings instead — currently the E2EE debug-log encryption passphrase (`src/commands/settings/profile.rs`).
- `edgee stats` (visible alias `report`) — prints session token counts and compression savings.
- `edgee statusline` — renders/manages the Claude Code statusline integration (see README.md's Statusline section for the install/doctor/fix flow).
- `edgee alias` — installs CLI PATH shims/shell aliases and desktop app wrappers (`cursor`, `copilot-vscode`) when the host app is installed.
- `edgee reset` — clears credentials.
- `edgee update` — compiled in only under the `self-update` feature.

Root flag: `-p/--profile` overrides the active profile. It must come **before** the subcommand (`edgee -p dev launch claude`) — deliberately not a clap `global` arg, so `launch`'s passthrough keeps a clean argv and `-p` reaches the agent (`edgee launch claude -p "prompt"` is Claude Code's `--print`).

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

- **Edition**: pinned to Rust edition 2021 in `Cargo.toml` — don't rely on edition-2024-only syntax.
- **`use` statement grouping**: order imports in blank-line-separated blocks:
  1. `std::...`
  2. external crates (crates.io dependencies)
  3. internal (`crate::...`, `super::...`)

  Apply the three-block grouping to new and edited code going forward.

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
