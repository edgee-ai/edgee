# edgee

A Claude Code mod that picks the model serving the current session through the
Edgee gateway. It calls the session reroute tools of the Edgee MCP server that
`edgee launch claude` injects, with the session id from `EDGEE_SESSION_ID`.

```
/edgee                    show the active reroute
/edgee list [filter]      models this session's API key can use
/edgee <model> [minutes]  reroute the session (default 60, max 1440)
/edgee off                route normally again
/edgee panel              open and focus the side pane
/edgee minimize           fold the pane into one line above the prompt
```

`<model>` matches an exact id (`qwen/qwen3-coder-next`), a bare name
(`qwen3-coder-next`) or a unique substring (`coder-next`). While a reroute is
active, the pane's header and the minimized band show `⇄ <model> until HH:MM`.

## Requests pane

Sessions start minimized by default. `/edgee panel` opens and focuses the pane:
docked beside the transcript on a wide terminal, waiting unplaced on a narrow
one until the window is wide enough. It shows:

- the Edgee session id and the active reroute;
- a model picker: a search field over a list of rows. "Claude Code's choice"
  (no reroute) is always first; on an empty search, the active and recently
  served models follow, then providers to browse; while searching, the first 8
  matches (provider and name prefixes first, newest version first). `●` marks
  the model in use. Picking reroutes at once, for 24 h; a spinner shows until
  the gateway confirms;
- session totals: requests, input, cached and output tokens;
- a Unicode donut chart of session cost savings, with estimated total saved,
  percentage cost reduction, and a green/purple/gray legend for compression,
  routing, and remaining cost. It stacks below the breakdown in narrow panes;
- session savings in USD: estimated rerouting savings and compression savings
  (input compression plus output brevity), refreshed every 30 seconds and after
  each main-agent turn. `—` means unavailable; a failed refresh marks the last
  known amounts as stale;
- per served model: request count and tokens;
- the last 30 API requests, newest first: time, served model, tokens in→out,
  duration, `↳` for subagents, and `asked <model>` when Claude Code asked for
  another model than the one that answered (a reroute at work).

Each Claude Code API request (`turn.step`) is recorded; the served model is the
one the gateway's response names. Requests made before the mod loaded are not
shown, and Claude Code side calls outside the turn loop are not seen.

### Keys

The pane takes the keyboard when it opens (or with `ctrl+x tab`, or a click).
Type to search; ↑/↓ (or Tab) move the highlight from the search field down
the rows, Enter picks the highlighted row (in the search
field, the first match). Picking a provider fills the search with it;
`m` (or the `[–]` button in the header) minimizes the pane;
Esc hands the keyboard back to the prompt.

### Minimized

Minimized, the pane closes and one line above the prompt stands in for it:
where requests go (`● direct`, or `⇄ <model> until HH:MM`), the last model
served, rerouting/compression savings, and the session's requests, tokens and
cache hit (as space permits). Its `[+]` button, or
`/edgee panel`, brings the pane back. The choice is remembered: subsequent
sessions restore the last minimized or expanded state.

## Shipping

`edgee launch claude` bundles this mod: the CLI compiles these files into its
binary (`src/commands/launch/claude_mods.rs`), writes them to
`~/.edgee/mods/edgee/` and loads them with `--plugin-dir`. It does so only
when the Edgee MCP server is injected, on Claude Code 2.1.287 or later, and it
pre-allows the three reroute tools the mod calls (`listSessionModels`,
`setSessionReroute`, `clearSessionReroute`). `EDGEE_MODS_DISABLED=1` turns it off.

## Limits

The mod replaces Edgee's legacy statusline. Without mod support or Edgee MCP,
there is no Edgee inline display. Gateway spend, reasoning-token totals, and
fallback alerts are not shown by this mod; consult the Edgee console.

Savings come from session analytics through `edgee stats --json --session <id>`,
using the CLI binary and profile that launched Claude. They include the whole
gateway session (including background calls) and may lag behind the request
timeline. Routing savings are estimates for requests with available pricing;
they are not inferred from model names. The CLI needs a signed-in profile to
read them. Older launchers without `EDGEE_CLI_PATH` show unavailable amounts.

- No tool reads a reroute back, so the pane reflects only what this mod
  set in this session. A reroute changed elsewhere leaves it stale.
- The reroute has no `sourceModel`: every request of the session goes to the
  target, Claude Code's background Haiku calls included.
- Claude Code's own `/model` and status line still show the model it asks for.
- The gateway may name the served model without its provider; the pane finds
  it in the model list. Token counts are what the gateway's response reports.

## Develop

```
claude --plugin-dir ./mods/edgee   # hot-reloads on save
claude plugin validate ./mods/edgee
claude plugin test ./mods/edgee

# Type-check: Claude Code writes its declarations to .claude-plugin/types/
# each time it loads the mod (any session with --plugin-dir does), which
# tsconfig.json extends.
npx -p typescript@5 tsc -p ./mods/edgee
```
