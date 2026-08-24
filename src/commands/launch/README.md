# Launch targets — naming rules

This document defines how `edgee launch <target>` names are chosen and how they
relate to credentials, transport, and the hidden `edgee relay` command.

Read this before adding a new agent.

## Why the catalogue matters

Wrapping the agent itself — with no code change on the user's side — is Edgee's
core differentiator against app-oriented gateways. Every target added here
widens the surface Edgee can route, meter, and govern. The catalogue below is
also the foundation the upcoming desktop app builds on: it will wrap the whole
local AI stack, CLI agents and desktop apps alike, from these same definitions.

So target names are long-lived public API. Renaming one breaks user aliases,
desktop wrappers, scripts, and docs at once. Get the name right the first time.

## Three layers (do not conflate them)

| Layer | What it is | Examples |
|---|---|---|
| **Launch target** | Public CLI name (`edgee launch …`) | `claude`, `cursor`, `copilot-vscode`, `claude-desktop`, later `copilot` |
| **Provider key** | Edgee credentials / console API key slot | `claude`, `claude_desktop`, `cursor`, `copilot` |
| **Transport** | How traffic reaches the gateway | CLI env-injection, MITM relay, … |

Users only see **launch targets**. Transport stays an implementation detail
(`edgee relay` remains `hide = true`). Several targets may share one provider
key (e.g. `copilot-vscode` today and future `copilot` CLI → provider `copilot`).
Surfaces may instead be split into their own backend agent when their usage
should be metered separately: `claude-desktop` is a dedicated `claude_desktop`
agent (own key + compression), distinct from `claude` (Claude Code).

## Naming convention

### 1. Bare name = primary surface

Usually the official CLI of that product:

```text
claude | codex | opencode | codebuddy | crush | pi | kimi | kilo | copilot | …
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
| `pi` | Pi CLI | `pi` |
| `kimi` | Kimi Code CLI | `kimi` |
| `kilo` | Kilo Code CLI | `kilo` |

### Apps & editors (relay today)

| Target | Product | Provider key | Notes |
|---|---|---|---|
| `cursor` | Cursor IDE | `cursor` | Relays the `cursor` binary |
| `copilot-vscode` | GitHub Copilot in VS Code | `copilot` | Relays `code`; aliases: `vscode-copilot`, `vscode`, `code` |
| `claude-desktop` | Claude Desktop | `claude_desktop` | Launches the Claude app bundle behind the relay; dedicated agent (own key + Claude compression flavor), **not** shared with `claude` (Claude Code) |
| `codex-desktop` | ChatGPT desktop app | `codex` | **No relay.** Its backend is a bundled `codex app-server` reading `$CODEX_HOME/config.toml`; the Edgee provider is written there, the app is launched detached, and the file is reverted ~10s later. See below. |

### `codex-desktop` — config patch, not relay

The ChatGPT desktop app embeds Codex (`ChatGPT.app/Contents/Resources/codex`,
launched as `app-server` over stdio) and honors `base_url` + `http_headers` from a
`model_providers` entry — the very settings `launch codex` passes as `-c` overrides.
So this target needs no proxy, no MITM CA and no system-keychain trust.

Three constraints are load-bearing, each established empirically:

- **The config file is the only lever.** The app supplies its own argv
  (`-c features.code_mode_host=true app-server`), so `-c` injection is impossible.
  `--profile <name>` is the design you'd want — it layers
  `$CODEX_HOME/<name>.config.toml` and never touches the user's `config.toml` — but
  it is **argv-only**: no `CODEX_PROFILE` env var exists, and the legacy
  `profile = "…"` config key is a fatal startup error in 0.147 (*"no longer
  supported; use `--profile`"*). If a future codex honors an env var for this, switch
  to it — it removes the only invasive part of this integration.
- **Never point `CODEX_HOME` at a private copy.** It *does* propagate into the spawned
  child, but a second home means a second `auth.json`, and the ChatGPT OAuth refresh
  token is single-use/rotating — whichever copy refreshes first invalidates the other
  and the user must sign in again. Symlink/hardlink does not help either: codex
  removes and atomically replaces the file. And auth cannot come from the
  environment — `CODEX_ACCESS_TOKEN` is an agent-identity slot, so it would force
  API-key billing instead of the user's ChatGPT plan.
- **The app parses `config.toml` once at startup and caches it.** This is what makes
  the whole thing cheap: the patch only needs to outlive the handoff.

So the lifecycle is patch → spawn **detached** → wait ~10s → revert → exit. The
command returns while the app keeps running on the cached settings, so the user's
`codex` CLI is unaffected once it returns, the terminal can be closed, and no crash
window can strand the patch.

The grace period is a timer, not a signal — we cannot observe codex reading the file.
It polls so the single-instance handoff is caught early. If a cold start ever exceeded
it, the app would read the reverted config and talk to OpenAI directly: no
compression, no metering, nothing broken, but also no warning.

Restore is **surgical**, not a rollback: codex rewrites `config.toml` at runtime
(project `trust_level`s, plugin state, `[desktop]` prefs), so the managed block is
stripped from the *current* file rather than restoring the backup snapshot, which
would discard the user's session. The backup remains for crash recovery and for the
case where codex reserializes the file and drops our marker comments.

The gateway must match the app's `User-Agent` (`Codex Desktop/…`) case-insensitively
for the Responses passthrough to fire; a case-sensitive `starts_with("codex")` sent
every desktop request down the keyed pipeline, which authenticates from
`Authorization` — the app's ChatGPT OAuth JWT — and 401'd.

## `pi` — additive provider in the user's own config

`opencode` and `crush` build a merged config in `$TMPDIR` and point the agent at
it (`OPENCODE_CONFIG`, `CRUSH_GLOBAL_CONFIG`), so the user's files are never
touched. Pi has no such lever, and the one that looks like it is a trap:
`PI_CODING_AGENT_DIR` relocates the **entire** agent directory — `models.json`
but also `auth.json`, `settings.json`, `keybindings.json`, `sessions/`,
`themes/`, `tools/`, `prompts/`, `bin/`, plus extension, skill and plugin
discovery. Pointing it at a temp dir launches pi with no history, no logins, no
settings and none of the user's plugins. `--models` is not an alternative: it
takes model *patterns* for Ctrl+P cycling, not a config path.

So this target writes into the real `~/.pi/agent/models.json`, under a single
`providers.edgee` key. Custom providers merge into pi's built-in catalog by
`provider + id`, so the block is purely **additive** — nothing the user already
had is overridden. That is what makes it safe to leave in place, and why there
is no patch-and-revert dance like `codex-desktop`: this adds a provider rather
than hijacking one the user depends on.

Three details are load-bearing:

- **Env references are `$NAME`, and this requires pi ≥ 0.79.4.** That release
  deliberately reversed the syntax (upstream #5661): before it, the *whole value*
  was the variable name (bare `EDGEE_API_KEY`) and an unset variable fell through
  to the literal string; from it, bare uppercase values are literals and `$NAME`
  is the only env reference. The two spellings are mutually exclusive — each is
  an inert literal on the other side of that boundary — and both fail
  identically, with the gateway answering 401 because it was handed
  `EDGEE_API_KEY` or `$EDGEE_API_KEY` as a credential. Check the pi version first
  when debugging a 401 here.
- **The Edgee key is therefore never written to disk** — the config stores the
  references `$EDGEE_API_KEY` / `$EDGEE_SESSION_ID` and launch supplies the
  values. This is strictly better than the OpenCode and Crush temp configs, which
  embed the key. The trade-off: a bare `pi` run sees the Edgee models but cannot
  authenticate them.
- **An empty gateway model list is fatal here, unlike for OpenCode.** A pi custom
  provider is defined by its models, so registering one with none opens the
  session on "No models available". `fetch_gateway_models` is best-effort and
  returns empty on any failure (an unreachable gateway, e.g. a dev profile
  pointing at a `localhost` port with nothing on it), so launch bails before
  writing rather than leaving a dead provider in the user's config.
- **`baseUrl` takes no `/v1`, and `api` is `anthropic-messages`.** Pi's built-in
  Anthropic provider is `https://api.anthropic.com` and pi appends
  `/v1/messages`, exactly like `ANTHROPIC_BASE_URL` for Claude Code. The gateway
  translates that shape for the whole catalog, so non-Anthropic models
  (`zai/…`, `openai/…`) route through it too.

Reasoning-capable models are declared with `reasoning: true` and a model-level
`thinkingLevelMap` generated from the catalog. Unsupported Pi levels are set to
`null`, so the picker hides and skips them; catalog `none` maps to Pi's `off`
slot. Because this provider always talks to the gateway rather than directly to
Anthropic, `compat.forceAdaptiveThinking` is enabled: Pi sends
`thinking.type=adaptive` plus the exact effort, and the gateway translates that
canonical control for whichever provider ultimately serves the request.

## `kimi` — the env-only channel Kimi Code leaves open

Kimi Code refuses to read provider credentials from the shell on purpose:
`api_key` / `base_url` come from `config.toml` (or its `[providers.<n>.env]`
sub-table), and `export KIMI_API_KEY=…` does nothing. So the lever every other
CLI target uses is closed here — except for one documented exception, the
`KIMI_MODEL_*` family, "an explicit channel that *does* read credentials from
the shell".

Setting `KIMI_MODEL_NAME` makes kimi synthesize a provider **and** a model alias
in memory, outranking `default_model` in `config.toml` and evaporating with the
process. Four variables are all this target needs:

```sh
KIMI_MODEL_NAME=moonshotai/kimi-k2.7-code   # also the enable switch
KIMI_MODEL_API_KEY=<edgee key>
KIMI_MODEL_BASE_URL=https://api.edgee.ai    # no /v1 — the SDK appends it
KIMI_MODEL_PROVIDER_TYPE=anthropic
```

That makes `kimi` the cleanest CLI target after `claude`: nothing is written to
the user's files, so there is no additive block to maintain (`pi`) and no
patch-and-revert dance (`codex-desktop`), and a bare `kimi` run afterwards is
completely unaffected.

Three details are load-bearing:

- **`base_url` takes no `/v1`.** `KIMI_MODEL_PROVIDER_TYPE=anthropic` selects
  Kimi's Anthropic Messages implementation, which appends `/v1/messages` itself
   — the same rule as `ANTHROPIC_BASE_URL` for Claude Code and `baseUrl` for pi.
- **The Edgee key travels as the provider credential, not as `x-edgee-api-key`.**
  The gateway resolves a key from `x-api-key` or `Authorization: Bearer`, and
  answers 401 to `x-edgee-api-key` on its own. `KIMI_MODEL_API_KEY` lands in the
  former, so it authenticates; there is nothing to add.
- **`KIMI_MODEL_NAME` is the enable switch, and a missing required variable is
  fatal.** Kimi fails at startup rather than quietly falling back to Moonshot, so
  a half-configured launch is loud instead of silently unmetered.

### Session attribution rides `KIMI_CODE_CUSTOM_HEADERS`

`KIMI_MODEL_*` carries no headers of its own, but Kimi Code 0.20.2 added
`KIMI_CODE_CUSTOM_HEADERS` — one `Name: Value` per line, applied to outbound LLM requests. Same
shape as `ANTHROPIC_CUSTOM_HEADERS`, so this target sends the full Edgee header set and sessions
group in the console exactly as they do for Claude Code.

The variable was **not** in the docs site's environment-variables reference at the time of writing —
only in the 0.20.2 release notes. Verified on the wire against a loopback server: `x-edgee-api-key`,
`x-edgee-session-id` and `x-edgee-repo` all arrive on `POST /v1/messages?beta=true`, alongside the
`x-api-key` that `KIMI_MODEL_API_KEY` produces. If a future release drops the variable, the fallback
is an additive `[providers.edgee]` block with `custom_headers` in the user's `config.toml`, the way
`pi.rs` writes `models.json` — but note `custom_headers` values take no `$NAME` env references, so
the key would then sit on disk, and a per-launch session id in a shared file races between
concurrent launches.

## `kilo` — inline config, nothing written anywhere

The Kilo Code CLI is an OpenCode fork: same config schema, same lowercase tool
names, same 32k `OUTPUT_TOKEN_MAX` clamp. So the obvious implementation is
`opencode.rs` with `KILO_CONFIG` swapped in for `OPENCODE_CONFIG`.

Kilo has a better lever. `KILO_CONFIG_CONTENT` takes the config as an **inline
JSON string** and sits near the top of the precedence chain:

```text
remote well-known → ~/.config/kilo/kilo.json → KILO_CONFIG → ./kilo.json
  → .kilo/kilo.json → KILO_CONFIG_CONTENT → managed      (deep-merged, later wins)
```

Two things follow, neither available to the `$TMPDIR` targets:

- **The key never touches disk.** `opencode` and `crush` write the Edgee key into
  a temp config and unlink it on exit; a crash in between leaves it on disk. Here
  it exists only in the child's environment.
- **Kilo does the merge.** `opencode.rs` has to find and parse the user's
  `opencode.json`/`.jsonc` itself — that is what its JSONC stripper is for — so it
  can re-emit their settings alongside ours. `KILO_CONFIG_CONTENT` is merged over
  whatever the user already has, so this target emits one `provider.edgee` key and
  reads nothing. Sitting above the project layer also means a repo-local
  `kilo.json` cannot shadow the Edgee provider.

**`KILO_CONFIG_DIR` is a trap**, and an unusually well-disguised one: the
configuration reference embedded in the binary calls it "appended to the search
list", which would make it the natural home for delivered skills and agents. The
binary resolves `config: KILO_CONFIG_DIR ?? Hc.config` — it **replaces** the
global config root, hiding the user's own commands, agents and skills. Where the
docs and the binary disagree, the binary wins.

Like `opencode` and `crush`, and unlike `claude` and `codex`, this target runs
**entirely on Edgee-supplied credentials** — it does not redirect an agent the
user already authenticated. `kilo auth` is left alone and unused on this path.

## Planned targets (same rules)

| Target | Product | Likely provider | Likely transport |
|---|---|---|---|
| `copilot` | GitHub Copilot CLI | `copilot` | CLI env |
| `claude-vscode` | Claude Code in VS Code | `claude` | Relay or native config |

## Checklist for a new target

1. Pick the **canonical launch name** with the rules above.
2. Add `src/commands/launch/<name>.rs` (use underscores in the
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
8. `edgee alias` covers both CLI and apps under one command:
   - CLI → PATH shims / shell aliases
   - Apps → desktop wrappers (macOS `.app`, Linux `.desktop`, Windows `.lnk`),
     **only if the host app is already installed**
   Register new app targets in `commands/alias/desktop.rs` (`AppSpec` + detection).

## Anti-patterns

- Exposing transport in the public UX (`edgee launch foo --relay` as the main path for apps that *only* work via relay — prefer a dedicated target that always relays).
- Using the IDE host name as the only public target when multiple products share that host (`vscode` alone).
- One subcommand with many surface flags (`--desktop`, `--vscode`, `--cli`).
- Forcing launch target name == provider key when multiple surfaces share billing/pipeline.
- Taking the bare product name for a non-CLI surface when a CLI is planned (`copilot` for VS Code).
