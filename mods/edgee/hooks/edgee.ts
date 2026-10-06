// Edgee Model: pick the model that serves this session through the Edgee
// gateway, using the session reroute tools of the Edgee MCP server that
// `edgee launch claude` injects.
//
//   /edgee                    show the active reroute
//   /edgee list [filter]      models this session's API key can use
//   /edgee <model> [minutes]  reroute the session (default 60, max 1440)
//   /edgee off                route normally again
//   /edgee panel              open and focus the side pane: a model selector, each API request and the model that served it
//   /edgee minimize           fold the pane into one line above the prompt (its [–] does too; the line's [+] unfolds it)

import type { EngineInterface, On, RenderElement, TextProps, BoxProps, TurnStepInput, TurnStepResult } from "claude-code";
import type { EdgeeModelTotals, EdgeePicker, EdgeeRequest, EdgeeReroute, EdgeeSavings } from "../types/index.d.ts";

type $ = EngineInterface;
type Elements = ReturnType<$["ui"]["resolve"]>;

const SERVER = "edgee";
const COMMAND = "edgee";
const DEFAULT_MINUTES = 60;
const PANE_MINUTES = 1440; // a pick in the pane lasts the longest reroute the gateway allows
const MAX_MINUTES = 1440;
const LIST_LIMIT = 40;
const PANE = "edgee-requests";
const RECENT = 30;
const OFF = "__off"; // the selector's "no reroute" option
const CATALOG_ATTEMPTS = 20;
const CATALOG_RETRY_MS = 1_000;
const ROWS = 8; // model rows under the filter; past that, typing narrows
const SUGGESTED_PROVIDERS = 8; // provider chips shown on an empty filter
const SPINNER = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_MS = 80;

// Held by the host, so they survive a hot reload of this file.
const reroute = { plugin: "edgee", key: "reroute" } as const;
const requests = { plugin: "edgee", key: "requests" } as const; // the last RECENT requests
const models = { plugin: "edgee", key: "models" } as const; // session totals per served model
const catalog = { plugin: "edgee", key: "catalog" } as const; // models the selector offers
const picker = { plugin: "edgee", key: "picker" } as const; // the selector's filter, duration and last notice
const minimized = { plugin: "edgee", key: "minimized" } as const; // the pane folded into a line above the prompt
const savings = { plugin: "edgee", key: "savings" } as const;
let refreshingSavings = false;
// Start minimized by default; remember an explicit choice across sessions.
const MINIMIZED_KEY = "minimized";

export function register(on: On) {
  on("session.start", async ($, e, next) => {
    const result = await next(e);
    await $.command.register({
      name: COMMAND,
      description: "Route this session to another model through Edgee",
      argumentHint: "[list [filter] | <model> [minutes] | off | panel | minimize]",
    });
    const folded = (await storedMinimized($)) !== false;
    await $.state.set(minimized, folded);
    // Start with the compact line unless the user last left the pane expanded.
    // Restoring the pane does not take the keyboard.
    if (!folded) await openPane($, { focus: false });
    if (await $.env.get("EDGEE_CLI_PATH")) {
      void refreshSavings($).catch(() => {}); // the host may unload during a refresh
      // Analytics can arrive after the turn ends; keep refreshing while idle too.
      $.clock.every(30_000, () => void refreshSavings($).catch(() => {}));
    }
    return result;
  });

  on("turn.complete", async ($, e, next) => {
    const result = await next(e);
    if (!e.agentId) {
      await current($); // drop the reroute once expired, so the pane and band redraw
      void refreshSavings($).catch(() => {});
    }
    return result;
  });

  // One step is one API request: record what Claude Code asked for and what answered.
  on("turn.step", async function* ($, e, next) {
    const startedAt = await $.clock.now();
    const result = yield* next(e);
    await record($, e, result, startedAt);
    return result;
  });

  on("ui.render", { component: "Pane", requestId: PANE }, async ($, e) => {
    const elements = $.ui.resolve(e);
    return paneView(elements, {
      sessionId: await $.env.get("EDGEE_SESSION_ID"),
      active: await current($),
      catalog: (await $.state.get(catalog)).value ?? [],
      picker: (await $.state.get(picker)).value ?? {},
      recent: (await $.state.get(requests)).value ?? [],
      totals: (await $.state.get(models)).value ?? {},
      savings: (await $.state.get(savings)).value,
      columns: e.props.bodyColumns,
      clickable: e.viewport?.isFullscreen === true,
      isFocused: e.props.isFocused,
    });
  });

  // The selector's elements: hooks rather than closures, so the work has `$`.
  // Each passes first, then acts: the work redraws the pane, and the element
  // `next` hands the event to would no longer be the one that raised it.
  on("ui.input", { plugin: "edgee", element: "filter" }, async ($, e, next) => {
    const result = await next(e);
    await updatePicker($, { filter: e.value });
    if (e.kind === "submit") {
      // Enter picks the first match; on an empty filter it steps into the list.
      const { value: models = [] } = await $.state.get(catalog);
      const [first] = e.value.trim() ? matching(models, e.value.trim()) : [];
      if (first) await pickModel($, first);
      else if (!e.value.trim()) await $.ui.focus({ requestId: PANE, key: `pick:${OFF}` });
    }
    return result;
  });

  // The list's rows: plain Buttons, so ↑↓ (or Tab) walk them and Enter picks.
  on("ui.press", { plugin: "edgee", element: /^(pick|provider):/ }, async ($, e, next) => {
    const result = await next(e);
    const [kind, value = ""] = splitOnce(e.element, ":");
    if (kind === "pick") await pickModel($, value);
    else if (kind === "provider") {
      await updatePicker($, { filter: `${value}/` });
      await $.ui.focus({ requestId: PANE, key: "filter" });
    }
    return result;
  });

  // The pane's [–] folds it into one line above the prompt; that line's [+] unfolds it.
  on("ui.press", { plugin: "edgee", element: "minimize" }, async ($, e, next) => {
    const result = await next(e);
    await minimize($);
    return result;
  });

  on("ui.press", { plugin: "edgee", element: "expand" }, async ($, e, next) => {
    const result = await next(e);
    await openPane($);
    return result;
  });

  on("ui.render", { component: "AbovePrompt" }, async ($, e, next) => {
    if (e.props.hasSurvey || !(await $.state.get(minimized)).value) return next(e);
    return bandView($.ui.resolve(e), {
      active: await current($),
      catalog: (await $.state.get(catalog)).value ?? [],
      recent: (await $.state.get(requests)).value ?? [],
      totals: (await $.state.get(models)).value ?? {},
      savings: (await $.state.get(savings)).value,
      columns: e.props.bodyColumns,
      clickable: e.viewport?.isFullscreen === true,
    });
  });

  on("command.run", { command: COMMAND }, async ($, e) => {
    const [verb = "", ...rest] = e.args.trim().split(/\s+/).filter(Boolean);
    if (verb === "panel") return { text: await openPane($) };
    if (verb === "minimize") {
      await minimize($);
      return { text: `Edgee pane minimized: /${COMMAND} panel restores it.` };
    }
    const sessionId = await $.env.get("EDGEE_SESSION_ID");
    if (!sessionId) {
      return { text: "Not running under Edgee. Start Claude Code with `edgee launch claude`." };
    }
    try {
      if (verb === "") return { text: await describe($) };
      if (verb === "list") return { text: await list($, sessionId, rest.join(" ")) };
      if (verb === "off" || verb === "clear" || verb === "reset") return { text: await clear($, sessionId) };
      return { text: await set($, sessionId, verb, rest[0]) };
    } catch (err) {
      // No `edgee` server (MCP not enabled), or its reroute tools not allowed yet.
      return { text: `Edgee MCP call failed: ${errorText(err)}` };
    }
  });
}

async function describe($: $): Promise<string> {
  const active = await current($);
  if (!active) return `No reroute: requests go to the model Claude Code asks for.\nUsage: /${COMMAND} list [filter] · /${COMMAND} <model> [minutes] · /${COMMAND} off`;
  return `Rerouted to ${active.model} until ${clockTime(active.expiresAt)}.`;
}

async function list($: $, sessionId: string, filter: string): Promise<string> {
  const models = await availableModels($, sessionId);
  const needle = filter.toLowerCase();
  const hits = models.filter((m) => m.toLowerCase().includes(needle));
  if (hits.length === 0) return `No model matches "${filter}".`;
  if (!needle) {
    // 200+ models: summarise by provider instead of dumping them all.
    const byProvider = new Map<string, number>();
    for (const m of hits) {
      const provider = m.split("/")[0]!;
      byProvider.set(provider, (byProvider.get(provider) ?? 0) + 1);
    }
    const rows = [...byProvider].map(([p, n]) => `  ${p} (${n})`);
    return `${hits.length} models available. Narrow with /${COMMAND} list <filter>:\n${rows.join("\n")}`;
  }
  const shown = hits.slice(0, LIST_LIMIT).map((m) => `  ${m}`);
  const more = hits.length > LIST_LIMIT ? `\n  … ${hits.length - LIST_LIMIT} more, narrow the filter` : "";
  return `${shown.join("\n")}${more}`;
}

async function set($: $, sessionId: string, query: string, minutesArg: string | undefined): Promise<string> {
  const models = await availableModels($, sessionId);
  const match = resolveModel(models, query);
  if (match.error !== undefined) return match.error;

  const minutes = clampMinutes(minutesArg);
  if (minutes === null) return `Invalid duration "${minutesArg}": give minutes between 1 and ${MAX_MINUTES}.`;
  return applyReroute($, sessionId, match.model, minutes);
}

async function applyReroute($: $, sessionId: string, model: string, minutes: number): Promise<string> {
  await callEdgee($, "setSessionReroute", { sessionId, targetModel: model, durationMinutes: minutes });
  const expiresAt = (await $.clock.now()) + minutes * 60_000;
  await $.state.set(reroute, { model, expiresAt });
  const span = minutes % 60 === 0 ? `${minutes / 60} h` : `${minutes} min`;
  return `Session rerouted to ${model} for ${span} (until ${clockTime(expiresAt)}).`;
}

async function clear($: $, sessionId: string): Promise<string> {
  await callEdgee($, "clearSessionReroute", { sessionId });
  await $.state.set(reroute, null);
  return "Reroute cleared: requests go to the model Claude Code asks for.";
}

async function availableModels($: $, sessionId: string): Promise<string[]> {
  const value = (await callEdgee($, "listSessionModels", { sessionId })) as { models?: unknown } | null;
  return Array.isArray(value?.models) ? value.models.filter((m): m is string => typeof m === "string") : [];
}

// Exact id first, then the bare name without its provider, then a unique substring.
function resolveModel(models: string[], query: string): { model: string; error?: undefined } | { model?: undefined; error: string } {
  const q = query.toLowerCase();
  const exact = models.find((m) => m.toLowerCase() === q || m.toLowerCase().split("/")[1] === q);
  if (exact) return { model: exact };
  const partial = models.filter((m) => m.toLowerCase().includes(q));
  if (partial.length === 1) return { model: partial[0]! };
  if (partial.length === 0) return { error: `Unknown model "${query}". See /${COMMAND} list <filter>.` };
  const shown = partial.slice(0, 10).map((m) => `  ${m}`).join("\n");
  return { error: `"${query}" matches ${partial.length} models, be more specific:\n${shown}` };
}

function clampMinutes(arg: string | undefined): number | null {
  if (arg === undefined) return DEFAULT_MINUTES;
  const n = Number(arg);
  if (!Number.isInteger(n) || n < 1 || n > MAX_MINUTES) return null;
  return n;
}

// Calls one Edgee MCP tool and returns its result parsed from JSON when it is JSON.
async function callEdgee($: $, tool: string, args: Record<string, unknown>): Promise<unknown> {
  const result = await $.mcp.call(SERVER, tool, args);
  const text = (result.content ?? [])
    .flatMap((b) => (b.type === "text" && typeof b.text === "string" ? [b.text] : []))
    .join("\n");
  if (result.isError) throw new Error(text || `${tool} failed`);
  if (result.structuredContent !== undefined) return result.structuredContent;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

// The active reroute, or null once it has expired.
async function current($: $): Promise<EdgeeReroute | null> {
  const { value } = await $.state.get(reroute);
  if (!value) return null;
  if (value.expiresAt <= (await $.clock.now())) {
    await $.state.set(reroute, null);
    return null;
  }
  return value;
}

async function openPane($: $, { focus = true } = {}): Promise<string> {
  await setMinimized($, false);
  const opened = await $.ui.open({ id: PANE, title: "Edgee requests", ...(focus ? { focus: true } : {}) });
  // Not awaited: the MCP server may still be connecting; the picker shows a loading state meanwhile.
  void loadCatalog($);
  return opened.isPlaced ? "Edgee requests pane opened." : `Edgee requests pane waits: ${opened.reason}`;
}

async function minimize($: $): Promise<void> {
  await setMinimized($, true);
  await $.ui.close({ id: PANE });
}

async function setMinimized($: $, value: boolean): Promise<void> {
  if ((await $.state.get(minimized)).value === value) return;
  await $.state.set(minimized, value);
  try {
    await $.store.set(MINIMIZED_KEY, value);
  } catch {
    // A preference: losing it means the next session starts minimized.
  }
}

async function storedMinimized($: $): Promise<unknown> {
  try {
    return await $.store.get(MINIMIZED_KEY);
  } catch {
    return undefined;
  }
}

// Loads the selector's models; a failure leaves the requests view working.
// At session start the `edgee` MCP server is often still connecting, so a
// failed load retries every second for a while before it reports.
async function loadCatalog($: $, attemptsLeft = CATALOG_ATTEMPTS): Promise<void> {
  const sessionId = await $.env.get("EDGEE_SESSION_ID");
  if (!sessionId) return;
  try {
    await $.state.set(catalog, await availableModels($, sessionId));
    const { value } = await $.state.get(picker);
    if (value?.failed) await updatePicker($, { notice: undefined, failed: false });
  } catch (err) {
    if (attemptsLeft > 1) {
      $.clock.after(CATALOG_RETRY_MS, () => void loadCatalog($, attemptsLeft - 1));
      return;
    }
    await updatePicker($, { notice: `Edgee MCP call failed: ${errorText(err)}`, failed: true });
  }
}

// A pick in the pane: reroute to `model`, or clear on OFF; a spinner shows
// while the gateway applies it, then the outcome shows under the selector.
async function pickModel($: $, model: string): Promise<void> {
  const sessionId = await $.env.get("EDGEE_SESSION_ID");
  let notice: string;
  let failed = false;
  if (!sessionId) [notice, failed] = ["Not running under Edgee.", true];
  else {
    await updatePicker($, { pending: model, frame: 0, notice: undefined, failed: false });
    let frame = 0;
    const spin = $.clock.every(SPINNER_MS, () => void updatePicker($, { frame: ++frame }));
    try {
      if (model === OFF) notice = await clear($, sessionId);
      else {
        notice = await applyReroute($, sessionId, model, PANE_MINUTES);
      }
    } catch (err) {
      [notice, failed] = [`Edgee MCP call failed: ${errorText(err)}`, true];
    } finally {
      spin.cancel();
    }
  }
  await updatePicker($, { notice, failed, pending: undefined });
}

async function updatePicker($: $, change: Partial<EdgeePicker>): Promise<void> {
  const { value } = await $.state.get(picker);
  await $.state.set(picker, { ...value, ...change });
}

async function record($: $, e: TurnStepInput, result: TurnStepResult | undefined, startedAt: number): Promise<void> {
  const usage = result?.usage;
  const entry: EdgeeRequest = {
    at: startedAt,
    durationMs: (await $.clock.now()) - startedAt,
    requested: e.model,
    served: usage?.model ?? null,
    input: (usage?.input_tokens ?? 0) + (usage?.cache_creation_input_tokens ?? 0),
    cached: usage?.cache_read_input_tokens ?? 0,
    output: usage?.output_tokens ?? 0,
    subagent: Boolean(e.agentId),
  };
  const { value: recent = [] } = await $.state.get(requests);
  await $.state.set(requests, [...recent, entry].slice(-RECENT));

  const key = entry.served ?? "failed";
  const { value: totals = {} } = await $.state.get(models);
  const t: EdgeeModelTotals = totals[key] ?? { requests: 0, input: 0, cached: 0, output: 0 };
  await $.state.set(models, {
    ...totals,
    [key]: {
      requests: t.requests + 1,
      input: t.input + entry.input,
      cached: t.cached + entry.cached,
      output: t.output + entry.output,
    },
  });
}

// Use the CLI's profile-aware authentication; credentials never enter plugin state.
// Replace the snapshot each time: the API totals already cover the whole session.
async function refreshSavings($: $): Promise<void> {
  if (refreshingSavings) return;
  refreshingSavings = true;
  try {
    const binary = await $.env.get("EDGEE_CLI_PATH");
    const sessionId = await $.env.get("EDGEE_SESSION_ID");
    if (!binary || !sessionId) return;
    const profile = await $.env.get("EDGEE_PROFILE");
    const result = await $.process.run(
      [binary, ...(profile ? ["--profile", profile] : []), "stats", "--json", "--session", sessionId],
      { timeoutMs: 10_000, env: { EDGEE_NO_UPDATE_CHECK: "1" } },
    );
    if (result.exitCode !== 0 || result.isStdoutTruncated) throw new Error("Savings unavailable");
    const value = JSON.parse(result.stdout);
    const input = nano(value.total_token_cost_savings);
    const output = nano(value.total_output_cost_savings);
    await $.state.set(savings, {
      compression: input !== null && output !== null ? input + output : null,
      rerouting: nano(value.estimated_routing_savings),
      remaining: nano(value.total_cost),
    });
  } catch {
    const { value } = await $.state.get(savings);
    await $.state.set(savings, { compression: null, rerouting: null, remaining: null, ...value, stale: true });
  } finally {
    refreshingSavings = false;
  }
}

function nano(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

function dollars(nano: number | null | undefined): string {
  if (nano == null) return "—";
  if (nano > 0 && nano < 10_000_000) return "<$0.01";
  return `$${(nano / 1_000_000_000).toFixed(2)}`;
}

// The pane's palette: Edgee's accent, and one hue per provider so a model reads at a glance.
const ACCENT = "#A78BFA";
const PROVIDER_COLORS: Record<string, string> = {
  anthropic: "#D97757",
  openai: "#10A37F",
  google: "#4285F4",
  qwen: "#8B5CF6",
  deepseek: "#4D6BFE",
  mistral: "#FA520F",
  meta: "#3B82F6",
  xai: "#E5E7EB",
  zai: "#22D3EE",
  moonshotai: "#F472B6",
  minimax: "#F43F5E",
  nvidia: "#76B900",
  amazon: "#FF9900",
};

type PaneData = {
  sessionId: string | undefined;
  active: EdgeeReroute | null;
  catalog: string[];
  picker: EdgeePicker;
  recent: EdgeeRequest[];
  totals: Record<string, EdgeeModelTotals>;
  savings: EdgeeSavings | undefined;
  columns: number;
  clickable: boolean; // the surface reports mouse clicks (fullscreen only): buttons are drawn only there
  isFocused: boolean;
};

function paneView(elements: Elements, { sessionId, active, catalog, picker, recent, totals, savings, columns, clickable, isFocused }: PaneData): RenderElement {
  const { Box, Text, Button } = elements;
  const inner = Math.max(24, columns - 4); // inside a card's border and padding
  const span = (children: string, props: Omit<TextProps, "children"> = {}) => Text({ ...props, children });
  const row = (children: RenderElement[], props: Omit<BoxProps, "children"> = {}) => Box({ flexDirection: "row", ...props, children });
  const card = (children: RenderElement[], color: string) =>
    Box({ flexDirection: "column", borderStyle: "round", borderColor: color, paddingX: 1, children });
  const rule = (title: string) => row([span("── ", { color: ACCENT, dimColor: true }), span(title.toUpperCase(), { color: ACCENT, bold: true }), span(` ${"─".repeat(Math.max(0, columns - title.length - 4))}`, { color: ACCENT, dimColor: true })], { marginTop: 1 });

  // ── Header: who we are and where requests go.
  const lastAsked = [...recent].reverse().find((r) => !r.subagent)?.requested;
  const header = card(
    [
      row(
        [
          row([span("◆ EDGEE", { color: ACCENT, bold: true }), span(`  session ${sessionId ? sessionId.slice(0, 8) : "—"}`, { dimColor: true })]),
          row([
            picker.pending
              ? span(` ${spinner(picker)} APPLYING `, { backgroundColor: "#92400E", color: "white", bold: true })
              : active
                ? span(" ⇄ REROUTED ", { backgroundColor: "#7C3AED", color: "white", bold: true })
                : span(" ● DIRECT ", { backgroundColor: "#065F46", color: "white", bold: true }),
            ...(clickable ? [span(" "), Button({ key: "minimize", label: "–", onPress: () => {} })] : []),
          ]),
        ],
        { justifyContent: "space-between" },
      ),
      active
        ? row([
            span(fit(bare(lastAsked ?? "claude"), Math.floor(inner / 3)).trimEnd(), { dimColor: true, strikethrough: true }),
            span("  ━━▶  ", { color: ACCENT, bold: true }),
            span(active.model, { color: providerColor(active.model), bold: true }),
            span(`  ⏱ ${clockTime(active.expiresAt)}`, { dimColor: true }),
          ])
        : span(sessionId ? "No reroute: requests go to the model Claude Code asks for" : "Not under edgee launch: requests are only observed", { dimColor: true, italic: true }),
    ],
    active ? ACCENT : "gray",
  );

  // ── Totals as tiles, then how much of the input the prompt cache served.
  const sum = sumTotals(totals);
  const tile = (label: string, value: string, color: string) =>
    Box({ flexDirection: "column", flexGrow: 1, alignItems: "center", borderStyle: "single", borderColor: color, borderDimColor: true, children: [span(value, { color, bold: true }), span(label, { dimColor: true })] });
  const hit = cacheHit(sum);
  const barWidth = Math.max(10, inner - 16);
  const stats = [
    row([tile("requests", String(sum.requests), ACCENT), tile("input", short(sum.input), "cyan"), tile("cached", short(sum.cached), "green"), tile("output", short(sum.output), "yellow")]),
    row([span(" cache hit ", { dimColor: true }), span("█".repeat(Math.round(hit * barWidth)), { color: "green" }), span("░".repeat(barWidth - Math.round(hit * barWidth)), { dimColor: true }), span(` ${Math.round(hit * 100)}%`, { color: "green", bold: true })]),
    savingsCard(elements, savings, inner),
  ];

  // ── Route: the selector.
  const route: RenderElement[] = [];
  if (catalog.length > 0) {
    const filter = (picker.filter ?? "").trim().toLowerCase();
    if ("Input" in elements) {
      route.push(elements.Input({ key: "filter", label: "⌕", placeholder: "search 200+ models: qwen, opus, gpt-5…", value: picker.filter ?? "", submitLabel: "pick first", autoFocus: true, onSubmit: () => {} }));
      route.push(span("┄".repeat(inner), { color: ACCENT, dimColor: true }));
    }
    route.push(...modelList(elements, { catalog, filter, active, totals, pending: picker.pending, inner }));
  } else {
    const waiting = sessionId ? (picker.failed ? "Models unavailable: is the Edgee MCP enabled?" : "Loading models from the Edgee MCP…") : "Launch with `edgee launch claude` to pick a model";
    route.push(span(waiting, { dimColor: true, italic: true }));
  }
  if (picker.pending) {
    const target = picker.pending === OFF ? "Claude Code's choice" : picker.pending;
    route.push(row([span(`${spinner(picker)} `, { color: ACCENT, bold: true }), span(picker.pending === OFF ? "Clearing the reroute → " : "Rerouting to ", { dimColor: true }), span(target, { color: providerColor(target), bold: true }), span(" …", { dimColor: true })], { marginTop: 1 }));
  } else if (picker.notice) route.push(row([span(picker.failed ? "✗ " : "✓ ", { color: picker.failed ? "red" : "green", bold: true }), span(picker.notice, { color: picker.failed ? "red" : undefined, dimColor: !picker.failed })], { marginTop: 1 }));

  // ── Models served: a share bar per model, in its provider's color.
  const entries = Object.entries(totals).sort((a, b) => b[1].requests - a[1].requests);
  const nameWidth = Math.max(12, Math.min(34, inner - 30));
  const shareWidth = Math.max(6, inner - nameWidth - 25);
  const served = entries.length
    ? entries.map(([servedAs, t]) => {
        const model = withProvider(servedAs, catalog);
        const filled = Math.max(1, Math.round((t.requests / sum.requests) * shareWidth));
        const color = model === "failed" ? "red" : providerColor(model);
        return row([
          span("● ", { color }),
          span(fit(model, nameWidth), { color, bold: true }),
          span(` ${"▰".repeat(filled)}`, { color }),
          span("▱".repeat(shareWidth - filled), { dimColor: true }),
          span(` ${String(t.requests).padStart(3)} req `, { bold: true }),
          span(`${short(t.input + t.cached).padStart(6)}↑ ${short(t.output).padStart(5)}↓`, { dimColor: true }),
        ]);
      })
    : [span("none yet", { dimColor: true, italic: true })];

  // ── Recent requests: a timeline, newest first; hovering a row lights it.
  const reqWidth = Math.max(12, inner - 34);
  const timeline = recent.length
    ? [...recent].reverse().map((r, i) => {
        const served = r.served && withProvider(r.served, catalog);
        const color = served ? providerColor(served) : "red";
        const secs = r.durationMs / 1000;
        const pace = secs < 3 ? "green" : secs < 10 ? "yellow" : "red";
        const lines = [
          row([
            span(clockTime(r.at, true), { dimColor: true }),
            span(r.subagent ? "  ↳ " : "  ● ", { color }),
            span(fit(served || "no response", reqWidth), { color, bold: !r.subagent }),
            span(` ${short(r.input + r.cached).padStart(6)}`, { color: "cyan" }),
            span("→", { dimColor: true }),
            span(short(r.output).padEnd(5), { color: "yellow" }),
            span(`${secs.toFixed(1).padStart(5)}s`, { color: pace }),
          ]),
        ];
        if (r.served && !sameModel(r.requested, r.served)) {
          lines.push(row([span("          ╰ ", { dimColor: true }), span(`⇄ from ${r.requested}`, { color: ACCENT, dimColor: true, italic: true })]));
        }
        return Box({ key: `req-${i}`, flexDirection: "column", hover: { backgroundColor: "#1F1B2E" }, children: lines });
      })
    : [span("none yet", { dimColor: true, italic: true })];

  return Box({
    flexDirection: "column",
    children: [
      header,
      ...stats,
      rule("Route"),
      card(route, "cyan"),
      rule("Models served"),
      Box({ flexDirection: "column", paddingX: 1, children: served }),
      rule("Recent requests"),
      Box({ flexDirection: "column", paddingX: 1, children: timeline }),
      row(isFocused
        ? [span("tab", { color: ACCENT, bold: true }), span(" move  ", { dimColor: true }), span("↑↓", { color: ACCENT, bold: true }), span(" choose  ", { dimColor: true }), span("⏎", { color: ACCENT, bold: true }), span(" pick  ", { dimColor: true }), span("/edgee minimize", { color: ACCENT, bold: true }), span("  ", { dimColor: true }), span("esc", { color: ACCENT, bold: true }), span(" back", { dimColor: true })]
        : [span("/edgee panel", { color: ACCENT, bold: true }), span(" focus  ", { dimColor: true }), span("/edgee minimize", { color: ACCENT, bold: true }), span(" fold", { dimColor: true })], { justifyContent: "center", marginTop: 1 }),
    ],
  });
}

const COMPRESSION_COLOR = "#55B98A";
const ROUTING_COLOR = "#B12ACB";
const REMAINING_COLOR = "#60566B";

function savingsCard({ Box, Text }: Elements, savings: EdgeeSavings | undefined, width: number): RenderElement {
  const compression = savings?.compression;
  const routing = savings?.rerouting;
  const remaining = savings?.remaining;
  const saved = compression != null && routing != null ? compression + routing : null;
  const baseline = saved != null && remaining != null ? saved + remaining : null;
  const percent = (value: number | null | undefined) => value != null && baseline != null && baseline > 0
    ? `${(100 * value / baseline).toFixed(1)}%` : "—";
  const legend = [
    ["Compression savings", compression, COMPRESSION_COLOR],
    ["Routing savings", routing, ROUTING_COLOR],
    ["Remaining cost", remaining, REMAINING_COLOR],
  ] as const;
  const details = Box({ flexDirection: "column", flexGrow: 1, children: [
    Text({ bold: true, children: dollars(saved) }),
    Text({ dimColor: true, children: "Estimated total saved" }),
    Text({ children: `${percent(saved)} cost reduction` }),
    ...legend.map(([label, value, color]) => Box({ flexDirection: "row", marginTop: 1, children: [
      Text({ color, children: "● " }),
      Text({ dimColor: true, children: `${label}  ` }),
      Text({ children: dollars(value) }),
      ...(label !== "Remaining cost" ? [Text({ dimColor: true, children: ` (${percent(value)})` })] : []),
    ] })),
  ] });
  return Box({ flexDirection: "column", borderStyle: "round", borderColor: ROUTING_COLOR, paddingX: 1, marginTop: 1, children: [
    Text({ bold: true, children: "Cost savings" }),
    Box({ flexDirection: width >= 64 ? "row" : "column", alignItems: "center", columnGap: 2, marginTop: 1, children: [
      details,
      savingsDonut({ Box, Text }, baseline && saved != null ? [compression! / baseline, routing! / baseline] : [0, 0], percent(saved)),
    ] }),
    Text({ dimColor: true, children: savings?.stale ? "Refresh unavailable · last known amounts" : "Session estimates · USD · — unavailable" }),
  ] });
}

// Braille provides 2×4 dots per terminal cell: a round ring without image support.
// Color each cell by its angle; all numeric labels use the exact savings amounts.
function savingsDonut({ Box, Text }: Pick<Elements, "Box" | "Text">, [compression, routing]: [number, number], label: string): RenderElement {
  const width = 21;
  const height = 9;
  const dots = [[1, 8], [2, 16], [4, 32], [64, 128]];
  const rows: RenderElement[] = [];
  for (let y = 0; y < height; y++) {
    const cells: RenderElement[] = [];
    for (let x = 0; x < width; x++) {
      if (y === 4 && x === 7) {
        cells.push(Text({ bold: true, children: label.padStart(6).padEnd(7) }));
        x += 6;
        continue;
      }
      let mask = 0;
      for (let dy = 0; dy < 4; dy++) for (let dx = 0; dx < 2; dx++) {
        const px = (x * 2 + dx + 0.5 - width) / width;
        const py = (y * 4 + dy + 0.5 - height * 2) / (height * 2);
        const radius = px * px + py * py;
        if (radius >= 0.52 && radius <= 1) mask |= dots[dy]![dx]!;
      }
      const angle = (Math.atan2((x + 0.5 - width / 2) / width, -(y + 0.5 - height / 2) / height) / (2 * Math.PI) + 1) % 1;
      const color = angle < compression ? COMPRESSION_COLOR : angle < compression + routing ? ROUTING_COLOR : REMAINING_COLOR;
      cells.push(Text({ color, children: mask ? String.fromCharCode(0x2800 + mask) : " " }));
    }
    rows.push(Box({ flexDirection: "row", children: cells }));
  }
  return Box({ flexDirection: "column", width, flexShrink: 0, children: rows });
}

// The models containing `filter`, those whose provider or name starts with it
// first ("qwen" lists qwen/… before deepseek/…-qwen-14b), newest version first.
function matching(models: string[], filter: string): string[] {
  const f = filter.toLowerCase();
  const rank = (m: string) => {
    const [provider = "", name = m] = m.toLowerCase().split("/");
    return provider.startsWith(f) ? 0 : name.startsWith(f) ? 1 : 2;
  };
  return models.filter((m) => m.toLowerCase().includes(f)).sort((a, b) => rank(a) - rank(b) || b.localeCompare(a, undefined, { numeric: true }));
}

function spinner(picker: EdgeePicker): string {
  return SPINNER[(picker.frame ?? 0) % SPINNER.length]!;
}

type ListData = { catalog: string[]; filter: string; active: EdgeeReroute | null; totals: Record<string, EdgeeModelTotals>; pending: string | undefined; inner: number };

// The picker's rows, each a plain Button: ↑↓ move the highlight, Enter picks.
// Claude Code's choice is always first. An empty filter offers the active and
// recently served models, then providers to narrow by; a filter, its matches.
function modelList({ Box, Text, Button }: Elements, { catalog, filter, active, totals, pending, inner }: ListData): RenderElement[] {
  const span = (children: string, props: Omit<TextProps, "children"> = {}) => Text({ ...props, children });
  const chosen = pending ?? active?.model ?? OFF;
  const option = (value: string, label: string, color: string, note = "") =>
    Box({
      flexDirection: "row",
      children: [
        span(value === chosen ? "● " : "○ ", { color: value === chosen ? "green" : color, bold: value === chosen }),
        Button({ key: `pick:${value}`, label: fit(label, Math.max(12, inner - 4 - note.length)).trimEnd(), plain: true, dimColor: value !== chosen, onPress: () => {} }),
        span(note, { dimColor: true }),
      ],
    });
  const rows = [option(OFF, "Claude Code's choice", "green", "  no reroute")];

  if (!filter) {
    const served = Object.keys(totals).filter((m) => m !== "failed").map((m) => withProvider(m, catalog));
    const suggested = [...new Set([...(active ? [active.model] : []), ...served])].filter((m) => catalog.includes(m) || m === active?.model);
    for (const m of suggested.slice(0, ROWS)) rows.push(option(m, m, providerColor(m), m === active?.model ? "  active" : "  used"));
    const providers = [...countBy(catalog, (m) => m.split("/")[0] ?? m)].sort((a, b) => b[1] - a[1]).slice(0, SUGGESTED_PROVIDERS);
    rows.push(
      Box({
        flexDirection: "row",
        flexWrap: "wrap",
        columnGap: 1,
        marginTop: 1,
        children: [span("browse", { dimColor: true }), ...providers.map(([p, n]) => Button({ key: `provider:${p}`, label: `${p} ${n}`, plain: true, onPress: () => {} }))],
      }),
    );
    return rows;
  }

  const hits = matching(catalog, filter);
  if (hits.length === 0) return [...rows, span(`  no model matches "${filter}"`, { color: "red", italic: true })];
  for (const m of hits.slice(0, ROWS)) rows.push(option(m, m, providerColor(m), m === active?.model ? "  active" : ""));
  if (hits.length > ROWS) rows.push(span(`  +${hits.length - ROWS} more · keep typing to narrow`, { dimColor: true, italic: true }));
  return rows;
}

function countBy<T>(items: T[], key: (item: T) => string): Map<string, number> {
  const counts = new Map<string, number>();
  for (const item of items) counts.set(key(item), (counts.get(key(item)) ?? 0) + 1);
  return counts;
}

function splitOnce(text: string, sep: string): [string, string] {
  const at = text.indexOf(sep);
  return at < 0 ? [text, ""] : [text.slice(0, at), text.slice(at + sep.length)];
}

type BandData = Pick<PaneData, "active" | "catalog" | "recent" | "totals" | "savings" | "columns" | "clickable">;

// The minimized pane: one line above the prompt, where requests go and what they cost.
function bandView({ Box, Text, Button }: Elements, { active, catalog, recent, totals, savings, columns, clickable }: BandData): RenderElement {
  const sum = sumTotals(totals);
  const lastServed = [...recent].reverse().find((r) => r.served)?.served;
  const served = lastServed ? withProvider(lastServed, catalog) : undefined;
  const facts = [`saved ⇄ ${dollars(savings?.rerouting)} est. / compression ${dollars(savings?.compression)}${savings?.stale ? " (stale)" : ""}`, `${sum.requests} req`, `${short(sum.input + sum.cached)}↑ ${short(sum.output)}↓`, `${Math.round(cacheHit(sum) * 100)}% cache`].join(" · ");
  return Box({
    flexDirection: "row",
    justifyContent: "space-between",
    width: columns,
    children: [
      Box({
        flexDirection: "row",
        flexShrink: 1,
        children: [
          Text({ color: ACCENT, bold: true, children: "◆ Edgee  " }),
          active
            ? Text({ color: ACCENT, bold: true, children: `⇄ ${bare(active.model)} until ${clockTime(active.expiresAt)}` })
            : Text({ color: "green", children: "● direct" }),
          ...(served ? [Text({ dimColor: true, children: " · " }), Text({ color: providerColor(served), children: bare(served) })] : []),
          Text({ dimColor: true, wrap: "truncate-end", children: ` · ${facts}` }),
        ],
      }),
      Box({
        flexDirection: "row",
        flexShrink: 0,
        children: [
          clickable
            ? Button({ key: "expand", label: "+", hotkey: "o", onPress: () => {} })
            : Text({ dimColor: true, children: "/edgee panel" }),
        ],
      }),
    ],
  });
}

function sumTotals(totals: Record<string, EdgeeModelTotals>): EdgeeModelTotals {
  return Object.values(totals).reduce(
    (s, t) => ({ requests: s.requests + t.requests, input: s.input + t.input, cached: s.cached + t.cached, output: s.output + t.output }),
    { requests: 0, input: 0, cached: 0, output: 0 },
  );
}

// The share of the input the prompt cache served.
function cacheHit({ input, cached }: EdgeeModelTotals): number {
  return input + cached > 0 ? cached / (input + cached) : 0;
}

// The gateway may name the served model without its provider (`qwen3-coder-next`):
// find it in the catalog to label and color it like the rest.
function withProvider(model: string, catalog: string[]): string {
  return model.includes("/") ? model : (catalog.find((m) => m.endsWith(`/${model}`)) ?? model);
}

function providerColor(model: string): string {
  return PROVIDER_COLORS[model.split("/")[0] ?? ""] ?? (model.startsWith("claude") ? "#D97757" : "white");
}

function bare(model: string): string {
  return model.replace(/\[.*\]$/, "").split("/").pop() ?? model;
}

// `claude-opus-5-5[1m]` and `anthropic/claude-opus-5-5-20260101` are the same model.
function sameModel(requested: string, served: string): boolean {
  const a = bare(requested).toLowerCase();
  const b = bare(served).toLowerCase();
  return a.startsWith(b) || b.startsWith(a);
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function fit(text: string, width: number): string {
  return text.length > width ? `${text.slice(0, width - 1)}…` : text.padEnd(width);
}

function short(n: number): string {
  if (n >= 1_000_000) return `${+(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${+(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function clockTime(ms: number, seconds = false): string {
  const d = new Date(ms);
  const parts = [d.getHours(), d.getMinutes(), ...(seconds ? [d.getSeconds()] : [])];
  return parts.map((n) => String(n).padStart(2, "0")).join(":");
}
