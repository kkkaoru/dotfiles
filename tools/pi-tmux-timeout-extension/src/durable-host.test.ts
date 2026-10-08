// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import { MemoryStorage } from "@earendil-works/pi-durable";
import tmuxTimeoutExtension, { type TmuxExtensionHost } from "../index.ts";
import { DurableTmuxState } from "./durable-state.ts";
import { durableTmuxHost } from "./durable-host.ts";
import type { CompletionDeliveryContext } from "./delivery.ts";
import { createTmuxLaunch } from "./tmux.ts";
import { ACTIVE_DISPLAY_ENTRY_TYPE } from "./active-display.ts";

vi.mock("./cleanup.ts", () => ({
  ArtifactCleaner: class {
    start = vi.fn();
    stop = vi.fn();
  },
}));

vi.mock("node:fs/promises", () => ({ mkdir: vi.fn().mockResolvedValue(undefined) }));
vi.mock("proper-lockfile", () => ({ lock: vi.fn().mockResolvedValue(vi.fn()) }));
vi.mock("@earendil-works/pi-durable/storage/jsonl/node", () => ({
  openNodeJsonlStorage: vi.fn(async () => new MemoryStorage()),
}));

afterEach(() => {
  vi.restoreAllMocks();
});

interface Fixture {
  readonly host: TmuxExtensionHost;
  readonly original: TmuxExtensionHost;
  readonly context: CompletionDeliveryContext;
  readonly handlers: Map<string, (event: unknown, context?: CompletionDeliveryContext) => unknown>;
}

function fixture(file?: string): Fixture {
  const handlers = new Map<
    string,
    (event: unknown, context?: CompletionDeliveryContext) => unknown
  >();
  const original: TmuxExtensionHost = {
    appendEntry: vi.fn(),
    exec: vi.fn().mockResolvedValue({ code: 0, stdout: "", stderr: "" }),
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerTool: vi.fn(),
    registerCommand: vi.fn(),
    sendUserMessage: vi.fn(),
  };
  const context: CompletionDeliveryContext = {
    isIdle: () => true,
    sessionManager: {
      getEntries: () => [],
      getSessionId: () => "owner",
      getSessionFile: () => file,
    },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const host = durableTmuxHost(original);
  host.on("session_start", vi.fn());
  host.on("session_shutdown", vi.fn());
  return { host, original, context, handlers };
}

it("requires initialization and commits launch/completion before execution/delivery", async () => {
  const { host, original, context, handlers } = fixture();
  expect(() => host.sendUserMessage("not ready")).toThrow("not ready");
  await handlers.get("session_start")?.({}, context);
  const launch = createTmuxLaunch({
    command: "echo ok",
    id: 1,
    namespace: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  });
  await host.prepareLaunch?.(launch);
  expect(original.appendEntry).toHaveBeenCalledOnce();
  await host.exec("sh", ["-lc", "echo ok"]);
  expect(original.exec).toHaveBeenCalledOnce();
  const deliver = vi.fn();
  host.complete?.({ launch, completedAt: new Date().toISOString(), exitCode: 0 }, deliver);
  expect(host.pendingTasks?.()).toHaveLength(1);
  expect(deliver).not.toHaveBeenCalled();
  await host.flush?.();
  expect(deliver).toHaveBeenCalledOnce();
  expect(host.pendingTasks?.()).toStrictEqual([]);
  host.sendUserMessage("done", { deliverAs: "followUp" });
  expect(original.sendUserMessage).toHaveBeenCalledOnce();
  await handlers.get("session_shutdown")?.({}, context);
  await expect(host.exec("sh", [])).rejects.toThrow("not ready");
});

it("opens persistent storage under the session file and releases its exclusive lock", async () => {
  const { mkdir } = await import("node:fs/promises");
  const { lock } = await import("proper-lockfile");
  const release = vi.fn().mockResolvedValue(undefined);
  vi.mocked(lock).mockResolvedValueOnce(release);
  const { host, context, handlers } = fixture("/sessions/owner.jsonl");
  await handlers.get("session_start")?.({}, context);
  expect(mkdir).toHaveBeenCalledWith("/sessions/owner.jsonl.tmux-durable", {
    recursive: true,
    mode: 0o700,
  });
  const cancel = host.monitor?.(vi.fn());
  await host.flush?.();
  cancel?.();
  await handlers.get("session_shutdown")?.({}, context);
  expect(release).toHaveBeenCalledOnce();
});

it("rejects missing ownership and failed locks without launching a command", async () => {
  const { lock } = await import("proper-lockfile");
  const { context, handlers, original } = fixture("/sessions/owner.jsonl");
  await expect(handlers.get("session_start")?.({})).rejects.toThrow("Missing Pi");
  await expect(
    handlers.get("session_start")?.({}, { isIdle: context.isIdle, ui: context.ui }),
  ).rejects.toThrow("Pi session is required");
  await handlers.get("session_shutdown")?.({}, context);
  vi.mocked(lock).mockRejectedValueOnce(new Error("already locked"));
  await expect(handlers.get("session_start")?.({}, context)).rejects.toThrow("already locked");
  expect(original.exec).not.toHaveBeenCalled();
});

it("flushes display command persistence and lifecycle handlers", async () => {
  const { host, original, context, handlers } = fixture();
  await handlers.get("session_start")?.({}, context);
  host.registerCommand?.("display", {
    description: "test",
    handler: () => host.appendEntry?.(ACTIVE_DISPLAY_ENTRY_TYPE, { hidden: true }),
  });
  const definition =
    original.registerCommand === undefined
      ? undefined
      : vi.mocked(original.registerCommand).mock.calls[0]?.[1];
  await definition?.handler("hide", context);
  expect(original.appendEntry).toHaveBeenCalledOnce();
  host.on("agent_start", () => "started");
  expect(await handlers.get("agent_start")?.({}, context)).toBe("started");
  await handlers.get("session_shutdown")?.({}, context);
});

it("loads the production entry point and restores the same owning session", async () => {
  const { original, context, handlers } = fixture();
  tmuxTimeoutExtension(original);
  await handlers.get("session_start")?.({}, context);
  expect(original.registerTool).toHaveBeenCalledOnce();
  await handlers.get("session_shutdown")?.({}, context);
});

it("suppresses a queued completion on shutdown and fails on a lost lock", async () => {
  const { lock } = await import("proper-lockfile");
  const { host, context, handlers } = fixture("/sessions/owner.jsonl");
  await handlers.get("session_start")?.({}, context);
  const launch = createTmuxLaunch({
    command: "echo ok",
    id: 1,
    namespace: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  });
  const deliver = vi.fn();
  host.complete?.({ launch, completedAt: new Date().toISOString(), exitCode: 0 }, deliver);
  await handlers.get("session_shutdown")?.({}, context);
  expect(deliver).not.toHaveBeenCalled();
  await handlers.get("session_start")?.({}, context);
  const options = vi.mocked(lock).mock.calls.at(-1)?.[1];
  const error = Object.assign(new Error("lock lost"), { code: "ECOMPROMISED" });
  if (typeof options === "object") {
    options.onCompromised?.(error);
  }
  expect(context.ui.notify).toHaveBeenCalledWith(
    "Pi Durable tmux stopped: Error: lock lost",
    "error",
  );
  await expect(host.flush?.()).rejects.toThrow("lock lost");
  await expect(handlers.get("session_shutdown")?.({}, context)).rejects.toThrow("lock lost");
});

it("fails visibly if its monitor is unavailable", async () => {
  const opened = vi.spyOn(DurableTmuxState, "open");
  const { host, context, handlers } = fixture();
  await handlers.get("session_start")?.({}, context);
  const state: DurableTmuxState | undefined = await opened.mock.results[0]?.value;
  if (state === undefined) {
    throw new Error("Missing state");
  }
  state.monitor = undefined;
  expect(() => host.monitor?.(vi.fn())).toThrow("monitor is not ready");
  await handlers.get("session_shutdown")?.({}, context);
});

it("initializes once for an early agent event and waits for storage before handlers", async () => {
  const { host, context, handlers } = fixture();
  const opened = Promise.withResolvers<DurableTmuxState>();
  const state = await DurableTmuxState.open(new MemoryStorage(), vi.fn());
  const open = vi.spyOn(DurableTmuxState, "open").mockReturnValueOnce(opened.promise);
  const start = vi.fn();
  const settled = vi.fn();
  host.on("session_start", start);
  host.on("agent_settled", settled);
  const early = handlers.get("agent_settled")?.({}, context);
  const starting = handlers.get("session_start")?.({ reason: "startup" }, context);
  expect(settled).not.toHaveBeenCalled();
  opened.resolve(state);
  await Promise.all([early, starting]);
  expect(open).toHaveBeenCalledOnce();
  expect(start).toHaveBeenCalledOnce();
  expect(settled).toHaveBeenCalledOnce();
  await handlers.get("session_shutdown")?.({}, context);
  await handlers.get("agent_settled")?.({}, context);
  expect(settled).toHaveBeenCalledOnce();
  expect(open).toHaveBeenCalledOnce();
});

it("does not flush closed state after an in-flight event and can initialize the next session", async () => {
  const { host, context, handlers } = fixture();
  const entered = Promise.withResolvers<null>();
  const finish = Promise.withResolvers<null>();
  host.on("agent_settled", async () => {
    entered.resolve(null);
    await finish.promise;
  });
  const event = handlers.get("agent_settled")?.({}, context);
  await entered.promise;
  await handlers.get("session_shutdown")?.({}, context);
  finish.resolve(null);
  await event;
  await handlers.get("session_start")?.({ reason: "new" }, context);
  await host.flush?.();
  await handlers.get("session_shutdown")?.({}, context);
});

it("releases the persistent lock when the startup handler fails", async () => {
  const { lock } = await import("proper-lockfile");
  const release = vi.fn().mockResolvedValue(undefined);
  vi.mocked(lock).mockResolvedValueOnce(release);
  const { host, context, handlers } = fixture("/sessions/failing.jsonl");
  host.on("session_start", () => {
    throw new Error("restore handler failed");
  });
  await expect(handlers.get("session_start")?.({}, context)).rejects.toThrow(
    "restore handler failed",
  );
  expect(release).toHaveBeenCalledOnce();
  expect(() => host.sendUserMessage("unsafe")).toThrow("not ready");
});

it("does not reopen a stopped session for a late command", async () => {
  const { host, original, context, handlers } = fixture();
  const command = vi.fn();
  host.registerCommand?.("display", { description: "test", handler: command });
  const definition =
    original.registerCommand === undefined
      ? undefined
      : vi.mocked(original.registerCommand).mock.calls[0]?.[1];
  await handlers.get("session_start")?.({}, context);
  await handlers.get("session_shutdown")?.({}, context);
  await expect(definition?.handler("hide", context)).rejects.toThrow("stopped");
  expect(command).not.toHaveBeenCalled();
});

it("does not execute a shell command if shutdown happens while persistence is flushing", async () => {
  const { host, original, context, handlers } = fixture();
  await handlers.get("session_start")?.({}, context);
  const flushed = Promise.withResolvers<null>();
  vi.spyOn(DurableTmuxState.prototype, "flush").mockImplementationOnce(async () => {
    await flushed.promise;
  });
  const execution = host.exec("sh", ["-lc", "echo unsafe"]);
  await handlers.get("session_shutdown")?.({}, context);
  flushed.resolve(null);
  await expect(execution).rejects.toThrow("stopped");
  expect(original.exec).not.toHaveBeenCalled();
});

it("waits for shutdown before opening the next session and closes only once", async () => {
  const { host, context, handlers } = fixture();
  const release = Promise.withResolvers<null>();
  const stopping = vi.fn(async () => {
    await release.promise;
  });
  host.on("session_shutdown", stopping);
  await handlers.get("session_start")?.({}, context);
  const shutdown = handlers.get("session_shutdown")?.({}, context);
  const duplicate = handlers.get("session_shutdown")?.({}, context);
  const started = vi.fn();
  const starting = Promise.resolve(
    handlers.get("session_start")?.({ reason: "new" }, context),
  ).then(started);
  await Promise.resolve();
  expect(started).not.toHaveBeenCalled();
  release.resolve(null);
  await Promise.all([shutdown, duplicate, starting]);
  expect(stopping).toHaveBeenCalledOnce();
  await host.flush?.();
  await handlers.get("session_shutdown")?.({}, context);
});

it("rejects an early event without session context", async () => {
  const { host, handlers } = fixture();
  host.on("agent_start", vi.fn());
  await expect(handlers.get("agent_start")?.({})).rejects.toThrow(
    "Missing Pi tmux session context",
  );
});

it("suppresses an event waiting for initialization when shutdown starts", async () => {
  const { host, context, handlers } = fixture();
  const opened = Promise.withResolvers<DurableTmuxState>();
  const state = await DurableTmuxState.open(new MemoryStorage(), vi.fn());
  vi.spyOn(DurableTmuxState, "open").mockReturnValueOnce(opened.promise);
  const handler = vi.fn();
  host.on("agent_settled", handler);
  const event = handlers.get("agent_settled")?.({}, context);
  const shutdown = handlers.get("session_shutdown")?.({}, context);
  opened.resolve(state);
  await Promise.all([event, shutdown]);
  expect(handler).not.toHaveBeenCalled();
});

it("does not flush a closed state after an asynchronous display command", async () => {
  const { host, original, context, handlers } = fixture();
  const entered = Promise.withResolvers<null>();
  const finish = Promise.withResolvers<null>();
  host.registerCommand?.("display", {
    description: "test",
    handler: async () => {
      entered.resolve(null);
      await finish.promise;
    },
  });
  const definition =
    original.registerCommand === undefined
      ? undefined
      : vi.mocked(original.registerCommand).mock.calls[0]?.[1];
  const command = definition?.handler("hide", context);
  await entered.promise;
  await handlers.get("session_shutdown")?.({}, context);
  finish.resolve(null);
  await expect(command).resolves.toBeUndefined();
});

it("registers startup before an immediate shutdown so storage cannot reopen afterward", async () => {
  const { host, context, handlers } = fixture();
  const opened = Promise.withResolvers<DurableTmuxState>();
  const state = await DurableTmuxState.open(new MemoryStorage(), vi.fn());
  const close = vi.spyOn(state, "close");
  vi.spyOn(DurableTmuxState, "open").mockReturnValueOnce(opened.promise);
  const start = handlers.get("session_start")?.({}, context);
  const stop = handlers.get("session_shutdown")?.({}, context);
  opened.resolve(state);
  await Promise.all([start, stop]);
  expect(close).toHaveBeenCalledOnce();
  expect(() => host.sendUserMessage("late")).toThrow("not ready");
});
