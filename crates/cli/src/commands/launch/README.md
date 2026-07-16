# Launch targets — naming rules

This document defines how `edgee launch <target>` names are chosen and how they
relate to credentials, transport, and the hidden `edgee relay` command.

Read this before adding a new agent.

## Three layers (do not conflate them)

| Layer | What it is | Examples |
|---|---|---|
| **Launch target** | Public CLI name (`edgee launch …`) | `claude`, `cursor`, `copilot-vscode`, later `copilot` / `claude-desktop` |
| **Provider key** | Edgee credentials / console API key slot | `claude`, `cursor`, `copilot` |
| **Transport** | How traffic reaches the gateway | CLI env-injection, MITM relay, … |

Users only see **launch targets**. Transport stays an implementation detail
(`edgee relay` remains `hide = true`). Several targets may share one provider
key (e.g. `copilot-vscode` today and future `copilot` CLI → provider `copilot`;
future `claude` + `claude-desktop` → provider `claude`).

## Naming convention

### 1. Bare name = primary surface

Usually the official CLI of that product:

```text
claude | codex | opencode | codebuddy | crush | pi | kilo | copilot | …
```

Reserve the bare product name for the CLI even if the CLI ships later. If only
an IDE/app surface exists today, use a suffixed name (see below) so the bare
name stays free.

### 2. Suffixed name = another surface of the same product

Pattern: `<product>-<surface>`

```text
copilot-vscode
claude-desktop
claude-vscode
codex-desktop
```

Do **not** overload the bare name with flags (`edgee launch claude --desktop`).
Each surface gets its own subcommand so help, scripts, and aliases stay obvious.

### 3. Distinct product = distinct bare name

When the product is not “another skin” of an existing CLI:

```text
cursor     # Cursor IDE (no separate CLI target yet)
```

Avoid ambiguous host-only names like bare `vscode` as a **canonical** target —
VS Code can host Copilot, Claude Code, etc. Prefer `copilot-vscode`, and later
`claude-vscode`, not a single `vscode` catch-all.

### 4. Aliases are optional discoverability only

Aliases may exist for muscle memory (`vscode-copilot` → `copilot-vscode`) but the
canonical name in docs, README tables, and new code is the one above.

Do **not** alias a reserved bare CLI name (`copilot`) to a suffixed surface.

## Current catalogue

### CLI agents (env → gateway)

| Target | Product | Provider key |
|---|---|---|
| `claude` | Claude Code CLI | `claude` |
| `codex` | Codex CLI | `codex` |
| `opencode` | OpenCode CLI | `opencode` |
| `codebuddy` | CodeBuddy CLI | `codebuddy` |
| `crush` | Crush CLI | `crush` |

### Apps & editors (relay today)

| Target | Product | Provider key | Notes |
|---|---|---|---|
| `cursor` | Cursor IDE | `cursor` | Relays the `cursor` binary |
| `copilot-vscode` | GitHub Copilot in VS Code | `copilot` | Relays `code`; aliases: `vscode-copilot`, `vscode`, `code` |

## Planned targets (same rules)

| Target | Product | Likely provider | Likely transport |
|---|---|---|---|
| `copilot` | GitHub Copilot CLI | `copilot` | CLI env |
| `pi` | Pi CLI | `pi` | CLI env |
| `kilo` | Kilo Code CLI | `kilo` | CLI env |
| `claude-desktop` | Claude Desktop | `claude` | Relay |
| `claude-vscode` | Claude Code in VS Code | `claude` | Relay or native config |
| `codex-desktop` | ChatGPT / Codex desktop app | `codex` | Relay |

## Checklist for a new target

1. Pick the **canonical launch name** with the rules above.
2. Add `crates/cli/src/commands/launch/<name>.rs` (use underscores in the
   module file, hyphens in the clap `name` when needed — e.g. `copilot_vscode.rs`
   → `copilot-vscode`).
3. Register it under `launch/mod.rs`:
   - CLI targets first, then Apps & editors.
   - Set `next_help_heading` on the first variant of each group.
   - Keep `about` text product-clear (`… CLI`, `… IDE`, …).
4. Map to the correct **provider key** in auth / credentials (may already exist).
5. Choose transport:
   - CLI with base URL / headers → follow `claude.rs` / `codex.rs`.
   - App that cannot be pointed at the gateway → thin wrapper calling
     `relay::run_for_agent("<canonical>")` (see `cursor.rs`, `copilot_vscode.rs`).
6. If relay: accept only the canonical name from launch; put legacy spellings in
   `relay::canonicalize_target` as aliases, not as new public targets. Never
   alias a reserved bare CLI name to an app surface.
7. Update the root `README.md` supported-setups table.
8. Shell aliases (`edgee alias`) are opt-in and usually CLI-only — do not add
   app aliases unless product explicitly wants them.

## Anti-patterns

- Exposing transport in the public UX (`edgee launch foo --relay` as the main path for apps that *only* work via relay — prefer a dedicated target that always relays).
- Using the IDE host name as the only public target when multiple products share that host (`vscode` alone).
- One subcommand with many surface flags (`--desktop`, `--vscode`, `--cli`).
- Forcing launch target name == provider key when multiple surfaces share billing/pipeline.
- Taking the bare product name for a non-CLI surface when a CLI is planned (`copilot` for VS Code).
