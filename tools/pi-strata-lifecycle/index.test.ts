import { homedir } from "node:os";
import path from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import strataLifecycle, {
  ensureScriptFor,
  providerOf,
  type StrataContext,
  type StrataHost,
} from "./index.ts";

const BIN = path.join(homedir(), ".local", "bin");
const TIMEOUT = 400_000;

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

let handler: (event: unknown, ctx: StrataContext) => Promise<void>;
let exec: ReturnType<typeof vi.fn<ExecFn>>;
let notify: ReturnType<typeof vi.fn<NotifyFn>>;
let ctx: StrataContext;

/** Register the extension against a fake host whose ensure script exits with `result`. */
function start(result: ExecResult): void {
  const host: StrataHost = {
    exec,
    on: (_event, fn) => {
      handler = fn;
    },
  };
  exec.mockResolvedValue(result);
  strataLifecycle(host);
}

beforeEach((): void => {
  exec = vi.fn<ExecFn>();
  notify = vi.fn<NotifyFn>();
  ctx = { ui: { notify } };
  handler = async (): Promise<void> => undefined;
});

describe("ensureScriptFor", () => {
  it("maps all strata providers to their ensure script", () => {
    expect(ensureScriptFor("strata")).toBe(path.join(BIN, "strata-local"));
    expect(ensureScriptFor("strata-mac")).toBe(path.join(BIN, "ensure-strata-mac"));
    expect(ensureScriptFor("strata-win")).toBe(path.join(BIN, "ensure-strata-win"));
  });

  it("ignores other providers and a missing provider", () => {
    expect(ensureScriptFor("omlx")).toBeUndefined();
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

describe("strataLifecycle", () => {
  it("starts the Mac's server when a strata model is selected", async () => {
    start({ code: 0, stderr: "", stdout: "" });
    await handler({ model: { provider: "strata" } }, ctx);
    expect(exec).toHaveBeenCalledWith(path.join(BIN, "strata-local"), [], { timeout: TIMEOUT });
    expect(notify).not.toHaveBeenCalled();
  });

  it("starts the GPU host when a strata-win model is selected", async () => {
    start({ code: 0, stderr: "", stdout: "" });
    await handler({ model: { provider: "strata-win" } }, ctx);
    expect(exec).toHaveBeenCalledWith(path.join(BIN, "ensure-strata-win"), [], {
      timeout: TIMEOUT,
    });
  });

  it("does nothing for a provider it does not own", async () => {
    start({ code: 0, stderr: "", stdout: "" });
    await handler({ model: { provider: "omlx" } }, ctx);
    expect(exec).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
  });

  it("warns with the last stderr line when the start fails", async () => {
    start({ code: 1, stderr: "noise\nstrata: cannot reach server", stdout: "" });
    await handler({ model: { provider: "strata" } }, ctx);
    expect(notify).toHaveBeenCalledWith(
      "strata start failed: strata: cannot reach server",
      "warning",
    );
  });

  it("falls back to the exit code when the failure says nothing", async () => {
    start({ code: 3, stderr: "", stdout: "" });
    await handler({ model: { provider: "strata" } }, ctx);
    expect(notify).toHaveBeenCalledWith("strata start failed: exit 3", "warning");
  });

  it("uses stdout when stderr is empty", async () => {
    start({ code: 1, stderr: "", stdout: "boom" });
    await handler({ model: { provider: "strata" } }, ctx);
    expect(notify).toHaveBeenCalledWith("strata start failed: boom", "warning");
  });
});
