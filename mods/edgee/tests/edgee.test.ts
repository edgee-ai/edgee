import type { RenderPropsOf } from "claude-code";
import { describe, expect, mock, test } from "claude-code/testing";

const PANE_PROPS = { title: "Edgee requests", isFocused: true, bodyColumns: 80, placement: "dock" } as unknown as RenderPropsOf["Pane"];

const BAND_PROPS = { hasSurvey: false, isWorking: false, maxRows: 10, bodyColumns: 120 } as unknown as RenderPropsOf["AbovePrompt"];

// Buttons are drawn only where clicks reach the mod.
const FULLSCREEN = { columns: 120, rows: 40, isFullscreen: true };

const MODELS = ["anthropic/claude-opus-5-5", "anthropic/claude-sonnet-5-5", "qwen/qwen3-coder-next", "qwen/qwen3-max"];

function text(value: unknown) {
  return { value: { content: [{ type: "text", text: JSON.stringify(value) }], isError: false } };
}

describe("edgee", () => {
  test("reroutes the session through the Edgee MCP server and clears it", async ($, on) => {
    // Hooks registered here run after the mod and stub what Claude Code would answer.
    const calls: { tool: string; args: Record<string, unknown> }[] = [];
    let now = Date.UTC(2026, 9, 2, 10, 0);
    on("session.start", ($, e) => ({ cwd: e.cwd }));
    on("command.register", ($, e) => ({ value: { command: e.name } }));
    on("env.get", () => ({ value: "sess-1" }));
    on("clock.now", () => ({ value: now }));
    const statuses: unknown[] = [];
    on("ui.status", ($, e) => {
      statuses.push(e.text);
      return { value: undefined };
    });
    on("turn.complete", () => ({ text: "" }));
    on("mcp.call", ($, e) => {
      calls.push({ tool: e.tool, args: e.args });
      if (e.server !== "edgee") throw new Error(`unexpected server ${e.server}`);
      if (e.tool === "listSessionModels") return text({ models: MODELS });
      return text({ ok: true });
    });

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);

    const ambiguous = await $.command.run({ command: "edgee", args: "qwen" } as any);
    expect(ambiguous.text).toMatch(/matches 2 models/);

    const set = await $.command.run({ command: "edgee", args: "coder 30" } as any);
    expect(set.text).toMatch(/rerouted to qwen\/qwen3-coder-next for 30 min/);
    expect(calls.at(-1)).toEqual({
      tool: "setSessionReroute",
      args: { sessionId: "sess-1", targetModel: "qwen/qwen3-coder-next", durationMinutes: 30 },
    });
    expect((await $.command.run({ command: "edgee", args: "" } as any)).text).toMatch(/Rerouted to qwen\/qwen3-coder-next/);
    // The pane and the band show the reroute: no status line under the prompt too.
    expect(statuses).toEqual([]);

    // Past expiry, the reroute is gone.
    now += 31 * 60_000;
    await $.turn.complete({ reason: "answer", answer: "ok", durationMs: 1 } as any);
    expect((await $.command.run({ command: "edgee", args: "" } as any)).text).toMatch(/No reroute/);

    const cleared = await $.command.run({ command: "edgee", args: "off" } as any);
    expect(cleared.text).toMatch(/Reroute cleared/);
    expect(calls.at(-1)).toEqual({ tool: "clearSessionReroute", args: { sessionId: "sess-1" } });
  });

  test("lists models by provider, then by filter", async ($, on) => {
    on("session.start", ($, e) => ({ cwd: e.cwd }));
    on("command.register", ($, e) => ({ value: { command: e.name } }));
    on("env.get", () => ({ value: "sess-1" }));
    on("clock.now", () => ({ value: 0 }));
    on("ui.status", () => ({ value: undefined }));
    on("mcp.call", () => text({ models: MODELS }));

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    const all = await $.command.run({ command: "edgee", args: "list" } as any);
    expect(all.text).toMatch(/4 models available/);
    expect(all.text).toMatch(/qwen \(2\)/);
    const some = await $.command.run({ command: "edgee", args: "list sonnet" } as any);
    expect(some.text).toBe("  anthropic/claude-sonnet-5-5");
  });

  test("explains when the session was not launched through Edgee", async ($, on) => {
    on("session.start", ($, e) => ({ cwd: e.cwd }));
    on("command.register", ($, e) => ({ value: { command: e.name } }));
    on("env.get", () => ({ value: undefined }));
    on("clock.now", () => ({ value: 0 }));
    on("ui.status", () => ({ value: undefined }));

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    const r = await $.command.run({ command: "edgee", args: "list" } as any);
    expect(r.text).toMatch(/Not running under Edgee/);
  });

  test("the pane lists each request and the model that served it", async ($, on) => {
    on("session.start", ($, e) => ({ cwd: e.cwd }));
    on("command.register", ($, e) => ({ value: { command: e.name } }));
    on("env.get", () => ({ value: "4eed308d-9830" }));
    on("clock.now", () => ({ value: Date.UTC(2026, 9, 2, 10, 0) }));
    on("ui.status", () => ({ value: undefined }));
    on("ui.open", () => ({ value: { isPlaced: true } }));
    on("mcp.call", () => text({ models: MODELS }));
    // The gateway may drop the provider: the pane finds it in the catalog.
    const served = ["claude-opus-5-5-20260101", "qwen3-coder-next"];
    on("turn.step", async function* ($, e) {
      return {
        turnId: e.turnId,
        index: e.index,
        answer: "",
        toolUses: [],
        stopReason: "end_turn",
        usage: { model: served[e.index], input_tokens: 1200, output_tokens: 300, cache_read_input_tokens: 40_000, cache_creation_input_tokens: 0 },
      } as any;
    });

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    const opened = await $.command.run({ command: "edgee", args: "panel" } as any);
    expect(opened.text).toMatch(/pane opened/);

    const ui = await $.ui.mount({ plugin: "edgee", surface: "terminal", component: "Pane", requestId: "edgee-requests", props: PANE_PROPS });
    expect(await ui.find({ type: "Text", text: /none yet/ })).toBeDefined();

    for (const index of [0, 1]) {
      const stream = $.turn.step({ turnId: "t1", index, model: "claude-opus-5-5[1m]", messageCount: 3 } as any);
      for await (const _ of stream);
    }
    // Tiles: requests, input, cached, output; then the cache-hit bar.
    for (const value of ["2", "2.4k", "80k", "600"]) {
      expect(await ui.find({ type: "Text", text: value })).toBeDefined();
    }
    expect(await ui.find({ type: "Text", text: / 97%/ })).toBeDefined();
    expect(await ui.find({ type: "Text", text: /^\s+1 req $/ })).toBeDefined();
    expect((await ui.findAll({ type: "Text", text: /^qwen\/qwen3-coder-next/ })).length).toBe(2); // served, timeline
    // Rerouted request: shows what Claude Code asked for. Same-model request: does not.
    expect((await ui.findAll({ type: "Text", text: /⇄ from claude-opus-5-5\[1m\]/ })).length).toBe(1);
    await ui.unmount();
  });

  test("the pane's selector filters, picks a duration and reroutes", async ($, on) => {
    const calls: { tool: string; args: Record<string, unknown> }[] = [];
    on("session.start", ($, e) => ({ cwd: e.cwd }));
    on("command.register", ($, e) => ({ value: { command: e.name } }));
    on("env.get", () => ({ value: "sess-1" }));
    on("clock.now", () => ({ value: Date.UTC(2026, 9, 2, 10, 0) }));
    on("ui.status", () => ({ value: undefined }));
    on("ui.open", () => ({ value: { isPlaced: true } }));
    on("clock.every", () => ({}) as never); // no spinner frames: the test reads the first one
    let gate: Promise<void> = Promise.resolve();
    on("mcp.call", async ($, e) => {
      calls.push({ tool: e.tool, args: e.args });
      if (e.tool === "listSessionModels") return text({ models: MODELS });
      await gate;
      return text({ ok: true });
    });

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    await $.command.run({ command: "edgee", args: "panel" } as any);
    const ui = await $.ui.mount({ plugin: "edgee", surface: "terminal", component: "Pane", requestId: "edgee-requests", props: PANE_PROPS });

    await ui.input({ key: "filter", text: "qwen", kind: "change" });
    expect(calls.at(-1)?.tool).toBe("listSessionModels"); // typing alone reroutes nothing
    const select = await ui.find({ type: "Select", key: "model" });
    expect((select?.props.options as { value: string }[]).map((o) => o.value)).toEqual(["__off", "qwen/qwen3-coder-next", "qwen/qwen3-max"]);
    // The folded dropdown hides the matches: the pane lists them under the filter,
    // Claude Code's choice pinned first whatever the filter.
    expect(await ui.find({ type: "Text", text: "● Claude Code's choice" })).toBeDefined();
    expect(await ui.find({ type: "Text", text: /2 matches/ })).toBeDefined();
    expect(await ui.find({ type: "Text", text: "3-coder-next" })).toBeDefined();
    await ui.input({ key: "filter", text: "nope", kind: "change" });
    expect(await ui.find({ type: "Text", text: /No model matches "nope"/ })).toBeDefined();
    await ui.input({ key: "filter", text: "qwen", kind: "change" });

    // While the gateway applies a pick, the pane says so.
    let answer!: () => void;
    gate = new Promise<void>((resolve) => (answer = resolve));
    await ui.select({ key: "duration", value: "240" });
    const picking = ui.select({ key: "model", value: "qwen/qwen3-max" });
    // Let the pick reach the gate: it crosses the host, so a real tick, not a microtask.
    await new Promise((resolve) => (globalThis as unknown as { setTimeout(fn: () => void, ms: number): void }).setTimeout(() => resolve(undefined), 10));
    expect(await ui.find({ type: "Text", text: /APPLYING/ })).toBeDefined();
    expect(await ui.find({ type: "Text", text: /Rerouting to/ })).toBeDefined();
    answer();
    await picking;
    expect(await ui.find({ type: "Text", text: /APPLYING/ })).toBeUndefined();
    expect(calls.at(-1)).toEqual({
      tool: "setSessionReroute",
      args: { sessionId: "sess-1", targetModel: "qwen/qwen3-max", durationMinutes: 240 },
    });
    expect(await ui.find({ type: "Text", text: /⇄ REROUTED/ })).toBeDefined();
    expect(await ui.find({ type: "Text", text: /Session rerouted to qwen\/qwen3-max for 240 min/ })).toBeDefined();

    // Enter in the filter picks its first match.
    await ui.input({ key: "filter", text: "coder", kind: "submit" });
    expect(calls.at(-1)?.args).toMatchObject({ targetModel: "qwen/qwen3-coder-next" });

    // Enter on an empty filter picks the pinned row: Claude Code's choice.
    await ui.input({ key: "filter", text: "", kind: "submit" });
    expect(calls.at(-1)?.tool).toBe("clearSessionReroute");
    await ui.input({ key: "filter", text: "max", kind: "submit" });
    expect(calls.at(-1)?.args).toMatchObject({ targetModel: "qwen/qwen3-max" });

    await ui.select({ key: "model", value: "__off" });
    expect(calls.at(-1)?.tool).toBe("clearSessionReroute");
    expect(await ui.find({ type: "Text", text: /● DIRECT/ })).toBeDefined();
    expect(await ui.find({ type: "Text", text: /Reroute cleared/ })).toBeDefined();
    await ui.unmount();
  });

  test("starts minimized by default and opens the pane on request", async ($, on) => {
    const opens: { id: string; focus?: true }[] = [];
    on("session.start", ($, e) => ({ cwd: e.cwd }));
    on("command.register", ($, e) => ({ value: { command: e.name } }));
    on("env.get", () => ({ value: "sess-1" }));
    on("clock.now", () => ({ value: 0 }));
    on("ui.status", () => ({ value: undefined }));
    on("mcp.call", () => text({ models: MODELS }));
    mock.store(on);
    on("ui.open", ($, e) => {
      opens.push({ id: e.id, focus: e.focus });
      return { value: { isPlaced: true } };
    });

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    expect(opens).toEqual([]);


    await $.command.run({ command: "edgee", args: "panel" } as any);
    expect(opens.at(-1)).toEqual({ id: "edgee-requests", focus: true });

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    expect(opens.at(-1)).toEqual({ id: "edgee-requests", focus: undefined });
  });

  test("retries the model list until the Edgee MCP server connects", async ($, on) => {
    let connected = false;
    on("session.start", ($, e) => ({ cwd: e.cwd }));
    on("command.register", ($, e) => ({ value: { command: e.name } }));
    on("env.get", () => ({ value: "sess-1" }));
    on("ui.status", () => ({ value: undefined }));
    on("ui.open", () => ({ value: { isPlaced: true } }));
    on("mcp.call", () => {
      if (!connected) throw new Error('no connected MCP tool "listSessionModels" on a server named "edgee"');
      return text({ models: MODELS });
    });
    const clock = mock.clock(on);

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    await $.command.run({ command: "edgee", args: "panel" } as any);
    const ui = await $.ui.mount({ plugin: "edgee", surface: "terminal", component: "Pane", requestId: "edgee-requests", props: PANE_PROPS });
    expect(await ui.find({ type: "Text", text: /Loading models/ })).toBeDefined();

    connected = true;
    await clock.advance(1_000);
    expect(await ui.find({ type: "Select", key: "model" })).toBeDefined();
    expect(await ui.find({ type: "Text", text: /MCP call failed/ })).toBeUndefined();
    await ui.unmount();
  });
  test("the pane minimizes to a line above the prompt and back, remembered across sessions", async ($, on) => {
    const opens: string[] = [];
    const closes: string[] = [];
    on("session.start", ($, e) => ({ cwd: e.cwd }));
    on("command.register", ($, e) => ({ value: { command: e.name } }));
    on("env.get", () => ({ value: "sess-1" }));
    on("clock.now", () => ({ value: Date.UTC(2026, 9, 2, 10, 0) }));
    on("ui.status", () => ({ value: undefined }));
    on("mcp.call", () => text({ models: MODELS }));
    on("ui.open", ($, e) => {
      opens.push(e.id);
      return { value: { isPlaced: true } };
    });
    on("ui.close", ($, e) => {
      closes.push(e.id);
    });
    on("ui.render", { component: "AbovePrompt" }, ($, e) => $.ui.resolve(e).Box({ children: [] })); // the band without the mod: empty
    mock.store(on);

    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    expect(opens).toEqual([]);
    await $.command.run({ command: "edgee", args: "panel" } as any);
    expect(opens).toEqual(["edgee-requests"]);
    const band = await $.ui.mount({ plugin: "edgee", surface: "terminal", component: "AbovePrompt", props: BAND_PROPS, viewport: FULLSCREEN });
    expect(await band.find({ type: "Text", text: /Edgee/ })).toBeUndefined(); // the pane is open: no line

    const pane = await $.ui.mount({ plugin: "edgee", surface: "terminal", component: "Pane", requestId: "edgee-requests", props: PANE_PROPS, viewport: FULLSCREEN });
    await pane.press({ key: "minimize" });
    expect(closes).toEqual(["edgee-requests"]);
    expect(await band.find({ type: "Text", text: /● direct/ })).toBeDefined();
    expect(await band.find({ type: "Button", key: "expand" })).toBeDefined();

    // A new session starts minimized: no pane, the line instead.
    await $.session.start({ surface: "terminal", isInteractive: true, cwd: "/work" } as any);
    expect(opens).toEqual(["edgee-requests"]);
    expect(await band.find({ type: "Text", text: /● direct/ })).toBeDefined();

    await band.press({ key: "expand" });
    expect(opens).toEqual(["edgee-requests", "edgee-requests"]);
    expect(await band.find({ type: "Text", text: /Edgee/ })).toBeUndefined();
    await pane.unmount();
    await band.unmount();
  });
});
