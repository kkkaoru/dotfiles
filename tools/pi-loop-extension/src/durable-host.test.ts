// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { MemoryStorage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import { afterEach, expect, it, vi } from "vitest";
import loopExtension, {
  type LoopCommandDefinition,
  type LoopExtensionHost,
  type LoopToolDefinition,
} from "../index.ts";
import { durableHost } from "./durable-host.ts";
import { DurableLoopStore } from "./durable-store.ts";
import type { LoopContext } from "./contracts.ts";
import { createLoopState, LOOP_STATE_ENTRY_TYPE } from "./state.ts";

const release = vi.hoisted(() => vi.fn(async () => undefined));
vi.mock("node:fs/promises", () => ({ mkdir: vi.fn() }));
vi.mock("proper-lockfile", () => ({ lock: vi.fn(async () => release) }));
vi.mock("@earendil-works/pi-durable/storage/jsonl/node", () => ({
  openNodeJsonlStorage: vi.fn(async () => new MemoryStorage()),
}));
afterEach(() => {
  vi.clearAllMocks();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it("loads the production entry, persists tools and closes a memory-only session", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const tools: LoopToolDefinition[] = [];
  const context: LoopContext = { isIdle: () => false, ui: { notify: vi.fn(), setStatus: vi.fn() } };
  loopExtension({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: (tool) => {
      tools.push(tool);
    },
    sendUserMessage: vi.fn(),
  });
  await handlers.get("session_start")?.({}, context);
  const start = tools.find((tool) => tool.name === "start_loop");
  if (start?.name !== "start_loop") {
    throw new Error("missing start tool");
  }
  await start.execute("start", { prompt: "test durable" }, undefined, undefined, context);
  const complete = tools.find((tool) => tool.name === "loop_complete");
  if (complete?.name !== "loop_complete") {
    throw new Error("missing complete tool");
  }
  await complete.execute("stop", { reason: "done" }, undefined, undefined, context);
  await handlers.get("agent_end")?.({}, context);
  await handlers.get("session_shutdown")?.({}, context);
  expect(mkdir).not.toHaveBeenCalled();
});

it("migrates a paused legacy session into locked durable storage without resuming it", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const commands: LoopCommandDefinition[] = [];
  const send = vi.fn();
  const legacy = createLoopState({
    jobs: [],
    nextId: 3,
    paused: true,
    pendingContinuations: ["pending"],
    runningContinuation: undefined,
  });
  const context: LoopContext = {
    isIdle: () => true,
    sessionManager: {
      getEntries: () => [{ type: "custom", customType: LOOP_STATE_ENTRY_TYPE, data: legacy }],
      getSessionFile: () => "/sessions/test.jsonl",
      getSessionId: () => "test",
    },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  loopExtension({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: (_name, command) => {
      commands.push(command);
    },
    registerTool: vi.fn(),
    sendUserMessage: send,
  });
  await handlers.get("session_start")?.({}, context);
  expect(mkdir).toHaveBeenCalledWith("/sessions/test.jsonl.loop-durable", {
    recursive: true,
    mode: 448,
  });
  expect(lock).toHaveBeenCalledOnce();
  expect(openNodeJsonlStorage).toHaveBeenCalledWith(
    "/sessions/test.jsonl.loop-durable",
    expect.anything(),
    { fsync: true },
  );
  expect(send).not.toHaveBeenCalled();
  await commands[0]?.handler("clear", context);
  await handlers.get("session_shutdown")?.({}, context);
  expect(release).toHaveBeenCalledOnce();
});

it("refuses writes before startup and never falls back when a storage lock is held", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const host: LoopExtensionHost = {
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
  };
  const bridge = durableHost(host);
  expect(() => bridge.host.sendUserMessage("before startup")).toThrow("not ready");
  expect(() => bridge.host.appendEntry?.("invalid", {})).toThrow("Invalid loop state");
  bridge.host.on("session_start", vi.fn());
  vi.mocked(lock).mockRejectedValueOnce(new Error("already locked"));
  const context: LoopContext = {
    isIdle: () => true,
    sessionManager: { getEntries: () => [], getSessionFile: () => "/sessions/test.jsonl" },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  await expect(handlers.get("session_start")?.({}, context)).rejects.toThrow("already locked");
  expect(() => bridge.scheduler.setInterval(vi.fn(), 100)).toThrow("not ready");
  expect(openNodeJsonlStorage).not.toHaveBeenCalled();
});

it("releases the storage lock after open failure", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const bridge = durableHost({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
  });
  bridge.host.on("session_start", vi.fn());
  vi.mocked(openNodeJsonlStorage).mockRejectedValueOnce(new Error("storage failed"));
  const context: LoopContext = {
    isIdle: () => true,
    sessionManager: { getEntries: () => [], getSessionFile: () => "/sessions/test.jsonl" },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  await expect(handlers.get("session_start")?.({}, context)).rejects.toThrow("storage failed");
  expect(release).toHaveBeenCalledOnce();
});

it("schedules and clears a wakeup through the actual durable entrypoint", async () => {
  vi.useFakeTimers();
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const tools: LoopToolDefinition[] = [];
  const commands: LoopCommandDefinition[] = [];
  const send = vi.fn();
  const context: LoopContext = { isIdle: () => true, ui: { notify: vi.fn(), setStatus: vi.fn() } };
  loopExtension({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: (_name, command) => {
      commands.push(command);
    },
    registerTool: (tool) => {
      tools.push(tool);
    },
    sendUserMessage: send,
  });
  await handlers.get("session_start")?.({}, context);
  const wake = tools.find((tool) => tool.name === "loop_wakeup");
  if (wake?.name !== "loop_wakeup") {
    throw new Error("missing wakeup tool");
  }
  await wake.execute("wake", { prompt: "check", delaySeconds: 60 }, undefined, undefined, context);
  await vi.advanceTimersByTimeAsync(60_100);
  expect(send).toHaveBeenCalledOnce();
  await commands[0]?.handler("pause", context);
  await commands[0]?.handler("clear", context);
  await handlers.get("session_shutdown")?.({}, context);
});

it("fails closed when the storage lock is compromised", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const bridge = durableHost({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
  });
  bridge.host.on("session_start", vi.fn());
  bridge.host.on("session_shutdown", vi.fn());
  const context: LoopContext = {
    isIdle: () => true,
    sessionManager: { getEntries: () => [], getSessionFile: () => "/sessions/test.jsonl" },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  await handlers.get("session_start")?.({}, context);
  vi.mocked(lock).mock.calls[0]?.[1]?.onCompromised?.(new Error("lock lost"));
  await expect(bridge.host.flush?.()).rejects.toThrow("lock lost");
  expect(context.ui.notify).toHaveBeenCalledWith(
    "Pi Durable loop stopped: Error: lock lost",
    "error",
  );
  await expect(handlers.get("session_shutdown")?.({}, context)).rejects.toThrow("lock lost");
  expect(release).toHaveBeenCalledOnce();
});

it("rejects an incomplete scheduler and mirrors committed state for compatibility", async () => {
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  store.scheduler = undefined;
  vi.spyOn(DurableLoopStore, "open").mockResolvedValueOnce(store);
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const appendEntry = vi.fn();
  const bridge = durableHost({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
    appendEntry,
  });
  bridge.host.on("session_start", vi.fn());
  bridge.host.on("session_shutdown", vi.fn());
  const context: LoopContext = { isIdle: () => true, ui: { notify: vi.fn(), setStatus: vi.fn() } };
  await handlers.get("session_start")?.({}, context);
  expect(() => bridge.scheduler.setInterval(vi.fn(), 100)).toThrow("scheduler is not ready");
  const state = createLoopState({
    jobs: [],
    nextId: 1,
    paused: false,
    pendingContinuations: [],
    runningContinuation: undefined,
  });
  bridge.host.appendEntry?.(LOOP_STATE_ENTRY_TYPE, state);
  expect(appendEntry).not.toHaveBeenCalled();
  await bridge.host.flush?.();
  expect(appendEntry).toHaveBeenCalledOnce();
  await handlers.get("session_shutdown")?.({}, context);
});

it("initializes once before an early turn and waits for storage before lifecycle events", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const bridge = durableHost({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
  });
  const context: LoopContext = { isIdle: () => true, ui: { notify: vi.fn(), setStatus: vi.fn() } };
  const opened = Promise.withResolvers<DurableLoopStore>();
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  const open = vi.spyOn(DurableLoopStore, "open").mockReturnValueOnce(opened.promise);
  const start = vi.fn();
  const end = vi.fn();
  bridge.host.on("session_start", start);
  bridge.host.on("agent_end", end);
  bridge.host.on("session_shutdown", vi.fn());
  const early = handlers.get("before_agent_start")?.({}, context);
  const ending = handlers.get("agent_end")?.({}, context);
  expect(end).not.toHaveBeenCalled();
  opened.resolve(store);
  await Promise.all([early, ending, handlers.get("session_start")?.({}, context)]);
  expect(open).toHaveBeenCalledOnce();
  expect(start).toHaveBeenCalledOnce();
  expect(end).toHaveBeenCalledOnce();
  await handlers.get("session_shutdown")?.({}, context);
});

it("does not flush a closed store when an in-flight event finishes after shutdown", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const bridge = durableHost({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
  });
  const context: LoopContext = { isIdle: () => true, ui: { notify: vi.fn(), setStatus: vi.fn() } };
  const entered = Promise.withResolvers<null>();
  const finish = Promise.withResolvers<null>();
  bridge.host.on("session_start", vi.fn());
  bridge.host.on("session_shutdown", vi.fn());
  bridge.host.on("agent_settled", async () => {
    entered.resolve(null);
    await finish.promise;
  });
  const event = handlers.get("agent_settled")?.({}, context);
  await entered.promise;
  await handlers.get("session_shutdown")?.({}, context);
  finish.resolve(null);
  await event;
  await handlers.get("session_start")?.({ reason: "new" }, context);
  await bridge.host.flush?.();
  await handlers.get("session_shutdown")?.({}, context);
});

it("waits for startup during shutdown and ignores pending or late agent events", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const bridge = durableHost({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
  });
  const context: LoopContext = { isIdle: () => true, ui: { notify: vi.fn(), setStatus: vi.fn() } };
  const opened = Promise.withResolvers<DurableLoopStore>();
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  vi.spyOn(DurableLoopStore, "open").mockReturnValueOnce(opened.promise);
  const end = vi.fn();
  bridge.host.on("agent_end", end);
  bridge.host.on("session_shutdown", vi.fn());
  const ending = handlers.get("agent_end")?.({}, context);
  const stopping = handlers.get("session_shutdown")?.({}, context);
  opened.resolve(store);
  await Promise.all([ending, stopping]);
  await handlers.get("agent_end")?.({}, context);
  await handlers.get("before_agent_start")?.({}, context);
  expect(end).not.toHaveBeenCalled();
  expect(() => bridge.host.sendUserMessage("late")).toThrow("not ready");
});

it("releases the persistent lock when the startup handler fails", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const bridge = durableHost({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: vi.fn(),
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
  });
  const context: LoopContext = {
    isIdle: () => true,
    sessionManager: { getEntries: () => [], getSessionFile: () => "/sessions/failing.jsonl" },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  bridge.host.on("session_start", () => {
    throw new Error("restore handler failed");
  });
  await expect(handlers.get("session_start")?.({}, context)).rejects.toThrow(
    "restore handler failed",
  );
  expect(release).toHaveBeenCalledOnce();
  expect(() => bridge.host.sendUserMessage("unsafe")).toThrow("not ready");
});

it("does not reopen a stopped session for a late command", async () => {
  const handlers = new Map<string, Parameters<LoopExtensionHost["on"]>[1]>();
  const commands: LoopCommandDefinition[] = [];
  const bridge = durableHost({
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerCommand: (_name, definition) => {
      commands.push(definition);
    },
    registerTool: vi.fn(),
    sendUserMessage: vi.fn(),
  });
  const context: LoopContext = { isIdle: () => true, ui: { notify: vi.fn(), setStatus: vi.fn() } };
  const command = vi.fn();
  bridge.host.registerCommand("loop", {
    description: "test",
    getArgumentCompletions: () => null,
    handler: command,
  });
  bridge.host.on("session_start", vi.fn());
  bridge.host.on("session_shutdown", vi.fn());
  await handlers.get("session_start")?.({}, context);
  await handlers.get("session_shutdown")?.({}, context);
  await expect(commands[0]?.handler("list", context)).rejects.toThrow("stopped");
  expect(command).not.toHaveBeenCalled();
});
