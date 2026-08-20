import { expect, it, vi } from "vitest";
import omlxLifecycleExtension, {
  createRunner,
  isProcessQuit,
  notifyOutcome,
  parseModelSwitchEvent,
  type OmlxContext,
  type OmlxLifecycleHost,
} from "./index.ts";

function createHost(exec: OmlxLifecycleHost["exec"]): {
  readonly host: OmlxLifecycleHost;
  readonly handlers: Map<string, (event: unknown, ctx: OmlxContext) => Promise<void> | void>;
} {
  const handlers = new Map<string, (event: unknown, ctx: OmlxContext) => Promise<void> | void>();
  const host: OmlxLifecycleHost = {
    exec,
    on: (event, handler): void => {
      handlers.set(event, handler);
    },
  };
  return { handlers, host };
}

function createContext(provider: string | undefined): OmlxContext {
  return {
    model: provider === undefined ? undefined : { provider },
    ui: { notify: vi.fn() },
  };
}

it("parses provider ids out of a real-shaped model_select event", () => {
  expect(
    parseModelSwitchEvent({
      model: { provider: "omlx" },
      previousModel: { provider: "anthropic" },
    }),
  ).toStrictEqual({ nextProvider: "omlx", previousProvider: "anthropic" });
});

it("parses a missing previousModel (startup selection) as an undefined previous provider", () => {
  expect(parseModelSwitchEvent({ model: { provider: "omlx" } })).toStrictEqual({
    nextProvider: "omlx",
    previousProvider: undefined,
  });
});

it("returns undefined for an event with no usable model field", () => {
  expect(parseModelSwitchEvent({})).toBeUndefined();
  expect(parseModelSwitchEvent(null)).toBeUndefined();
  expect(parseModelSwitchEvent("not-an-object")).toBeUndefined();
  expect(parseModelSwitchEvent({ model: "not-an-object" })).toBeUndefined();
  expect(parseModelSwitchEvent({ model: {} })).toBeUndefined();
  expect(parseModelSwitchEvent({ model: { provider: 123 } })).toBeUndefined();
});

it("adapts host.exec into the runtime's OmlxCommandRunner shape", async () => {
  const exec = vi.fn().mockResolvedValue({ code: 0, stderr: "", stdout: "ok" });
  const runner = createRunner({ exec });

  const result = await runner.run("/bin/true", ["--flag"], 5000);

  expect(exec).toHaveBeenCalledWith("/bin/true", ["--flag"], { timeout: 5000 });
  expect(result).toStrictEqual({ code: 0, stderr: "", stdout: "ok" });
});

it("stays silent for a successful outcome", () => {
  const ctx = createContext("omlx");
  notifyOutcome(ctx, { action: "start", ok: true });
  expect(ctx.ui.notify).not.toHaveBeenCalled();
});

it("stays silent for a none action", () => {
  const ctx = createContext("anthropic");
  notifyOutcome(ctx, { action: "none", ok: true });
  expect(ctx.ui.notify).not.toHaveBeenCalled();
});

it("warns with the failure reason when starting omlx fails", () => {
  const ctx = createContext("omlx");
  notifyOutcome(ctx, { action: "start", ok: false, reason: "missing /Applications/oMLX.app" });
  expect(ctx.ui.notify).toHaveBeenCalledWith(
    "omlx start skipped: missing /Applications/oMLX.app",
    "warning",
  );
});

it("warns with a fallback message when stopping omlx fails without a reason", () => {
  const ctx = createContext("omlx");
  notifyOutcome(ctx, { action: "stop", ok: false });
  expect(ctx.ui.notify).toHaveBeenCalledWith(
    "omlx idle-stop check skipped: unknown error",
    "warning",
  );
});

it("starts omlx on model_select when switching into omlx", async () => {
  const exec = vi.fn().mockResolvedValue({ code: 0, stderr: "", stdout: "" });
  const { handlers, host } = createHost(exec);

  omlxLifecycleExtension(host);
  const ctx = createContext("anthropic");
  await handlers.get("model_select")?.(
    { model: { provider: "omlx" }, previousModel: { provider: "anthropic" } },
    ctx,
  );

  expect(exec).toHaveBeenCalledWith(
    expect.stringContaining("ensure-omlx"),
    [],
    expect.objectContaining({ timeout: 200_000 }),
  );
});

it("ignores an unparsable model_select event without calling exec", async () => {
  const exec = vi.fn();
  const { handlers, host } = createHost(exec);

  omlxLifecycleExtension(host);
  await handlers.get("model_select")?.({}, createContext(undefined));

  expect(exec).not.toHaveBeenCalled();
});

it("identifies a real process quit but not reload/new/resume/fork reasons", () => {
  expect(isProcessQuit({ reason: "quit" })).toBe(true);
  expect(isProcessQuit({ reason: "reload" })).toBe(false);
  expect(isProcessQuit({ reason: "new" })).toBe(false);
  expect(isProcessQuit({})).toBe(false);
  expect(isProcessQuit(null)).toBe(false);
  expect(isProcessQuit("quit")).toBe(false);
});

it("nudges omlx-idle-stop on session_shutdown when quitting with omlx as the current provider", async () => {
  const exec = vi.fn().mockResolvedValue({ code: 0, stderr: "", stdout: "" });
  const { handlers, host } = createHost(exec);

  omlxLifecycleExtension(host);
  await handlers.get("session_shutdown")?.({ reason: "quit" }, createContext("omlx"));

  expect(exec).toHaveBeenCalledWith(
    expect.stringContaining("omlx-idle-stop"),
    [],
    expect.objectContaining({ timeout: 10_000 }),
  );
});

it("does nothing on session_shutdown when quitting with a non-omlx current provider", async () => {
  const exec = vi.fn();
  const { handlers, host } = createHost(exec);

  omlxLifecycleExtension(host);
  await handlers.get("session_shutdown")?.({ reason: "quit" }, createContext("anthropic"));

  expect(exec).not.toHaveBeenCalled();
});

it("does not nudge omlx-idle-stop when the shutdown is a reload/new/resume/fork replacing the running process", async () => {
  const exec = vi.fn();
  const { handlers, host } = createHost(exec);

  omlxLifecycleExtension(host);
  await handlers.get("session_shutdown")?.({ reason: "reload" }, createContext("omlx"));

  expect(exec).not.toHaveBeenCalled();
});

it("surfaces a warning notification when ensure-omlx fails during a model switch", async () => {
  const exec = vi.fn().mockResolvedValue({ code: 1, stderr: "missing app", stdout: "" });
  const { handlers, host } = createHost(exec);

  omlxLifecycleExtension(host);
  const ctx = createContext(undefined);
  await handlers.get("model_select")?.({ model: { provider: "omlx" } }, ctx);

  expect(ctx.ui.notify).toHaveBeenCalledWith("omlx start skipped: missing app", "warning");
});
