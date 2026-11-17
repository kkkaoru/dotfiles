import { homedir } from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import strataLifecycle, {
  ensureScriptFor,
  footerText,
  parseAwake,
  providerOf,
  type AwakeState,
  type StrataContext,
  type StrataHost,
} from "./index.ts";

const BIN = path.join(homedir(), ".local", "bin");
const TIMEOUT = 400_000;
const PROBE = path.join(BIN, "strata-awake");

interface ExecResult {
  readonly code: number;
  readonly stderr: string;
  readonly stdout: string;
}
type ExecFn = (
  command: string,
  args: string[],
  options?: { timeout?: number },
) => Promise<ExecResult>;
type NotifyFn = (message: string, level?: "error" | "info" | "warning") => void;

const ALL_COLD = "strata-win asleep\nstrata-mac asleep\nbonsai-mac asleep\nstrata-colab unknown\n";

let handlers: Map<string, (event: unknown, ctx: StrataContext) => Promise<void>>;
let exec: ReturnType<typeof vi.fn<ExecFn>>;
let notify: ReturnType<typeof vi.fn<NotifyFn>>;
let setStatus: ReturnType<typeof vi.fn<(key: string, text: string | undefined) => void>>;
let ctx: StrataContext;

/** Register the extension against a fake host; `exec` answers by command name. */
function start(result?: ExecResult): void {
  const probe = result ?? { code: 0, stderr: "", stdout: ALL_COLD };
  const host: StrataHost = {
    exec,
    on: (event, fn) => {
      handlers.set(event, fn);
    },
  };
  exec.mockImplementation(async (command: string) =>
    command === PROBE ? probe : { code: 0, stderr: "", stdout: "" },
  );
  strataLifecycle(host);
}

/** Fire one event the way pi does, using the context it would pass. */
async function fire(event: string, payload: unknown, context: StrataContext = ctx): Promise<void> {
  const handler = handlers.get(event);
  if (handler === undefined) {
    throw new Error(`no handler for ${event}`);
  }
  await handler(payload, context);
}

beforeEach((): void => {
  exec = vi.fn<ExecFn>();
  notify = vi.fn<NotifyFn>();
  setStatus = vi.fn<(key: string, text: string | undefined) => void>();
  handlers = new Map();
  ctx = { ui: { notify, setStatus } };
});

afterEach((): void => {
  vi.useRealTimers();
});

describe("ensureScriptFor", () => {
  it("maps all strata providers to their ensure script", () => {
    expect(ensureScriptFor("strata")).toBe(path.join(BIN, "strata-local"));
    expect(ensureScriptFor("strata-mac")).toBe(path.join(BIN, "ensure-strata-mac"));
    expect(ensureScriptFor("strata-win")).toBe(path.join(BIN, "ensure-strata-win"));
    expect(ensureScriptFor("bonsai-mac")).toBe(path.join(BIN, "ensure-bonsai-mac"));
  });

  it("ignores other providers and a missing provider", () => {
    expect(ensureScriptFor("omlx")).toBeUndefined();
    expect(ensureScriptFor("bonsai")).toBeUndefined();
    expect(ensureScriptFor(undefined)).toBeUndefined();
  });
});

describe("providerOf", () => {
  it("reads the provider of pi's model_select event", () => {
    expect(providerOf({ model: { provider: "strata-win" } })).toBe("strata-win");
  });

  it("returns undefined for shapes it does not understand", () => {
    expect(providerOf(undefined)).toBeUndefined();
    expect(providerOf("strata")).toBeUndefined();
    expect(providerOf({})).toBeUndefined();
    expect(providerOf({ model: null })).toBeUndefined();
    expect(providerOf({ model: {} })).toBeUndefined();
    expect(providerOf({ model: { provider: 7 } })).toBeUndefined();
  });
});

describe("parseAwake", () => {
  it("reads the provider states strata-awake prints", () => {
    expect(parseAwake(ALL_COLD)).toEqual({
      "strata-win": "asleep",
      "strata-mac": "asleep",
      "bonsai-mac": "asleep",
      "strata-colab": "unknown",
    });
  });

  it("skips empty and malformed lines", () => {
    expect(parseAwake("\nstrata-win awake\ngarbage\n\nstrata-mac sideways\n")).toEqual({
      "strata-win": "awake",
    });
  });

  it("accepts no output at all", () => {
    expect(parseAwake("")).toEqual({});
  });
});

describe("footerText", () => {
  const states: Readonly<Record<string, AwakeState>> = {
    "strata-win": "awake",
    "strata-mac": "asleep",
    "bonsai-mac": "asleep",
    "strata-colab": "unknown",
  };

  it("names every endpoint and the one that answers", () => {
    expect(footerText(states, new Map(), 0)).toBe(
      "awake: strata-win awake · strata-mac cold · bonsai-mac cold · strata-colab unknown",
    );
  });

  it("warns that the idle timer is about to stop an awake endpoint", () => {
    expect(footerText(states, new Map([["strata-win", 0]]), 9 * 60_000)).toBe(
      "awake: strata-win awake (idle stops it in ~1m) · strata-mac cold · bonsai-mac cold · strata-colab unknown",
    );
  });

  it("omits endpoints that have not been probed yet", () => {
    expect(footerText({ "strata-win": "awake" }, new Map(), 0)).toBe("awake: strata-win awake");
    expect(footerText({}, new Map(), 0)).toBeUndefined();
  });

  it("stays quiet about the idle timer while the endpoint was used recently", () => {
    expect(footerText(states, new Map([["strata-win", 0]]), 60_000)).toBe(
      "awake: strata-win awake · strata-mac cold · bonsai-mac cold · strata-colab unknown",
    );
  });
});

describe("strataLifecycle", () => {
  it("starts the Mac's server when a strata model is selected", async () => {
    start();
    await fire("model_select", { model: { provider: "strata" } });
    expect(exec).toHaveBeenCalledWith(path.join(BIN, "strata-local"), [], { timeout: TIMEOUT });
    expect(notify).not.toHaveBeenCalled();
  });

  it("starts the GPU host when a strata-win model is selected", async () => {
    start();
    await fire("model_select", { model: { provider: "strata-win" } });
    expect(exec).toHaveBeenCalledWith(path.join(BIN, "ensure-strata-win"), [], {
      timeout: TIMEOUT,
    });
  });

  it("does nothing for a provider it does not own", async () => {
    start();
    await fire("model_select", { model: { provider: "omlx" } });
    expect(exec).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
  });

  it("warns with the last stderr line when the start fails", async () => {
    start();
    exec.mockResolvedValue({ code: 1, stderr: "noise\nstrata: cannot reach server", stdout: "" });
    await fire("model_select", { model: { provider: "strata" } });
    expect(notify).toHaveBeenCalledWith(
      "strata start failed: strata: cannot reach server",
      "warning",
    );
  });

  it("falls back to the exit code when the failure says nothing", async () => {
    start();
    exec.mockResolvedValue({ code: 3, stderr: "", stdout: "" });
    await fire("model_select", { model: { provider: "strata" } });
    expect(notify).toHaveBeenCalledWith("strata start failed: exit 3", "warning");
  });

  it("shows the warm-up, then the endpoint it just started", async () => {
    start();
    await fire("model_select", { model: { provider: "strata-win" } });
    expect(setStatus.mock.calls).toEqual([
      ["strata", "strata-win: starting..."],
      ["strata", "awake: strata-win awake"],
    ]);
  });

  it("warms the server of the active model before a request", async () => {
    start();
    await fire(
      "before_provider_headers",
      { headers: {} },
      { model: { provider: "strata-win" }, ui: { notify, setStatus } },
    );
    expect(exec).toHaveBeenCalledWith(path.join(BIN, "ensure-strata-win"), [], {
      timeout: TIMEOUT,
    });
  });

  it("does not probe again right after a successful warm", async () => {
    start();
    const warmCtx: StrataContext = { model: { provider: "strata" }, ui: { notify, setStatus } };
    await fire("before_provider_headers", { headers: {} }, warmCtx);
    await fire("before_provider_headers", { headers: {} }, warmCtx);
    expect(
      exec.mock.calls.filter(([command]) => command === path.join(BIN, "strata-local")),
    ).toHaveLength(1);
  });

  it("retries a warm that failed on the next request", async () => {
    start();
    exec.mockResolvedValue({ code: 1, stderr: "down", stdout: "" });
    const warmCtx: StrataContext = { model: { provider: "strata" }, ui: { notify, setStatus } };
    await fire("before_provider_headers", { headers: {} }, warmCtx);
    await fire("before_provider_headers", { headers: {} }, warmCtx);
    expect(
      exec.mock.calls.filter(([command]) => command === path.join(BIN, "strata-local")),
    ).toHaveLength(2);
  });
});

describe("strataLifecycle status", () => {
  it("skips a request made without an active model", async () => {
    start();
    await fire("before_provider_headers", { headers: {} }, { ui: { notify, setStatus } });
    expect(exec).not.toHaveBeenCalled();
  });

  it("reports a probe that failed right after startup", async () => {
    start();
    exec.mockRejectedValue(new Error("boom"));
    await fire("session_start", {});
    await vi.waitFor(() => {
      expect(notify).toHaveBeenCalledWith("strata status: Error: boom", "warning");
    });
    await fire("session_shutdown", {});
  });

  it("publishes the endpoint states when the session starts", async () => {
    start();
    await fire("session_start", {});
    expect(exec).toHaveBeenCalledWith(PROBE, [], { timeout: 40_000 });
    expect(setStatus).toHaveBeenCalledWith(
      "strata",
      "awake: strata-win cold · strata-mac cold · bonsai-mac cold · strata-colab unknown",
    );
  });

  it("says when an endpoint went cold between polls", async () => {
    vi.useFakeTimers();
    start({
      code: 0,
      stderr: "",
      stdout: "strata-win awake\nstrata-mac asleep\nstrata-colab unknown\n",
    });
    await fire("session_start", {});
    exec.mockImplementation(async () => ({ code: 0, stderr: "", stdout: ALL_COLD }));
    await vi.advanceTimersByTimeAsync(60_000);
    expect(notify).toHaveBeenCalledWith(
      "strata-win: went cold; the next request starts it again",
      "info",
    );
  });

  it("stops polling when the session shuts down", async () => {
    vi.useFakeTimers();
    start();
    await fire("session_start", {});
    await fire("session_shutdown", {});
    exec.mockClear();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(exec).not.toHaveBeenCalled();
  });

  it("reports a probe that failed instead of throwing", async () => {
    vi.useFakeTimers();
    start();
    await fire("session_start", {});
    exec.mockRejectedValue(new Error("boom"));
    await vi.advanceTimersByTimeAsync(60_000);
    expect(notify).toHaveBeenCalledWith("strata status: Error: boom", "warning");
  });
});
