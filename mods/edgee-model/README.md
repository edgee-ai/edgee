# edgee-model

A Claude Code mod that picks the model serving the current session through the
Edgee gateway. It calls the session reroute tools of the Edgee MCP server that
`edgee launch claude` injects, with the session id from `EDGEE_SESSION_ID`.

```
/edgee-model                    show the active reroute
/edgee-model list [filter]      models this session's API key can use
/edgee-model <model> [minutes]  reroute the session (default 60, max 1440)
/edgee-model off                route normally again
/edgee-model panel              focus the side pane (it opens with the session)
```

`<model>` matches an exact id (`qwen/qwen3-coder-next`), a bare name
(`qwen3-coder-next`) or a unique substring (`coder-next`). While a reroute is
active, a status line under the prompt shows `⇄ Edgee: <model> until HH:MM`.

## Requests pane

The pane opens with the session, without taking the keyboard: docked beside the
transcript on a wide terminal, waiting unplaced on a narrow one until the window
is wide enough. `/edgee-model panel` focuses it. It shows:

- the Edgee session id and the active reroute;
- a model selector: a filter field, a model picker (the first 12 matches, plus
  "Claude Code's choice" to clear the reroute) and a duration picker. Picking a
  model reroutes at once;
- session totals: requests, input, cached and output tokens;
- per served model: request count and tokens;
- the last 30 API requests, newest first: time, served model, tokens in→out,
  duration, `↳` for subagents, and `asked <model>` when Claude Code asked for
  another model than the one that answered (a reroute at work).

Each Claude Code API request (`turn.step`) is recorded; the served model is the
one the gateway's response names. Requests made before the mod loaded are not
shown, and Claude Code side calls outside the turn loop are not seen.

### Keys

The pane takes the keyboard when it opens (or with `ctrl+x tab`, or a click).
Then Tab moves between the filter, the model and the duration; typing in the
filter narrows the models and Enter there picks the first match; ↑/↓ move
through a picker's options and Enter picks;
Esc hands the keyboard back to the prompt.

## Shipping

`edgee launch claude` bundles this mod: the CLI compiles these files into its
binary (`src/commands/launch/claude_mods.rs`), writes them to
`~/.edgee/mods/edgee-model/` and loads them with `--plugin-dir`. It does so only
when the Edgee MCP server is injected, on Claude Code 2.1.287 or later, and it
pre-allows the three reroute tools the mod calls (`listSessionModels`,
`setSessionReroute`, `clearSessionReroute`). `EDGEE_MODS_DISABLED=1` turns it off.

## Limits

- No tool reads a reroute back, so the status line reflects only what this mod
  set in this session. A reroute changed elsewhere leaves it stale.
- The reroute has no `sourceModel`: every request of the session goes to the
  target, Claude Code's background Haiku calls included.
- Claude Code's own `/model` and status line still show the model it asks for.
- The gateway may name the served model without its provider; the pane finds
  it in the model list. Token counts are what the gateway's response reports.

## Develop

```
claude --plugin-dir ./mods/edgee-model   # hot-reloads on save
claude plugin validate ./mods/edgee-model
claude plugin test ./mods/edgee-model

# Type-check: Claude Code writes its declarations to .claude-plugin/types/
# each time it loads the mod (any session with --plugin-dir does), which
# tsconfig.json extends.
npx -p typescript@5 tsc -p ./mods/edgee-model
```
