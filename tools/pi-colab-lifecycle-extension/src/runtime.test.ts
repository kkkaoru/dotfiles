import { expect, it, vi } from "vitest";
import {
  ColabLifecycleRuntime,
  type Config,
  type Deps,
  type RunResult,
  type State,
} from "./runtime.ts";

interface Harness {
  readonly runtime: ColabLifecycleRuntime;
  readonly run: ReturnType<typeof vi.fn<Deps["run"]>>;
  readonly spawnDetached: ReturnType<typeof vi.fn<Deps["spawnDetached"]>>;
  readonly store: { state: State };
}

const config: Config = {
  colabCommand: "colab",
  upScript: "up.sh",
  upTimeoutMs: 1000,
  watcherScript: "watch.sh",
};

interface HarnessInput {
  readonly initial: State;
  readonly sessions: string;
  readonly upCode: number;
  readonly urlAfterUp?: string;
}

function setup(input: HarnessInput): Harness {
  const store = { state: input.initial };
  const run = vi.fn<Deps["run"]>(async (command): Promise<RunResult> => {
    if (command === "colab") {
      return { code: 0, stdout: input.sessions, stderr: "" };
    }
    if (input.urlAfterUp !== undefined) {
      store.state = { ...store.state, url: input.urlAfterUp };
    }
    return { code: input.upCode, stdout: "", stderr: "" };
  });
  const spawnDetached = vi.fn<Deps["spawnDetached"]>();
  const deps: Deps = {
    run,
    spawnDetached,
    readState: () => store.state,
    writeState: (next) => {
      store.state = next;
    },
    now: () => 42,
  };
  return { runtime: new ColabLifecycleRuntime(deps, config), run, spawnDetached, store };
}

it("reuses a live session without starting Colab", async () => {
  const harness = setup({
    initial: { url: "https://a.trycloudflare.com" },
    sessions: "[qwen] A100",
    upCode: 0,
  });
  const outcome = await harness.runtime.ensureUp();
  expect(outcome).toStrictEqual({ ok: true, started: false, url: "https://a.trycloudflare.com" });
  expect(harness.run).not.toHaveBeenCalledWith("up.sh", [], 1000);
  expect(harness.spawnDetached).toHaveBeenCalledWith("watch.sh", []);
  expect(harness.store.state.lastRequestAt).toBe(42);
});

it("starts Colab when the session is gone", async () => {
  const harness = setup({
    initial: { url: "https://old.trycloudflare.com" },
    sessions: "[?] orphan",
    upCode: 0,
    urlAfterUp: "https://new.trycloudflare.com",
  });
  const outcome = await harness.runtime.ensureUp();
  expect(outcome).toStrictEqual({ ok: true, started: true, url: "https://new.trycloudflare.com" });
  expect(harness.run).toHaveBeenCalledWith("up.sh", [], 1000);
});

it("starts Colab when no URL is recorded", async () => {
  const harness = setup({
    initial: {},
    sessions: "",
    upCode: 0,
    urlAfterUp: "https://new.trycloudflare.com",
  });
  const outcome = await harness.runtime.ensureUp();
  expect(outcome.started).toBe(true);
});

it("reports a failed start without throwing", async () => {
  const harness = setup({ initial: {}, sessions: "", upCode: 1 });
  const outcome = await harness.runtime.ensureUp();
  expect(outcome).toStrictEqual({
    ok: false,
    started: true,
    reason: "colab start failed (exit 1)",
  });
  expect(harness.spawnDetached).not.toHaveBeenCalled();
});

it("shares one start between concurrent callers", async () => {
  const harness = setup({
    initial: {},
    sessions: "",
    upCode: 0,
    urlAfterUp: "https://new.trycloudflare.com",
  });
  await Promise.all([harness.runtime.ensureUp(), harness.runtime.ensureUp()]);
  const upCalls = harness.run.mock.calls.filter(([command]) => command === "up.sh");
  expect(upCalls).toHaveLength(1);
});

it("treats a failing sessions listing as not running", async () => {
  const harness = setup({
    initial: { url: "https://a.trycloudflare.com" },
    sessions: "",
    upCode: 0,
  });
  harness.run.mockResolvedValueOnce({ code: 1, stdout: "[qwen]", stderr: "" });
  const outcome = await harness.runtime.ensureUp();
  expect(outcome.started).toBe(true);
});

it("shows remaining compute units and caches the lookup", async () => {
  const harness = setup({
    initial: {},
    sessions: "Current balance: 50 compute units\nUsage rate: 5/hr",
    upCode: 0,
  });
  const first = await harness.runtime.usageText();
  const second = await harness.runtime.usageText();
  expect(first).toBe("Colab 50.0 CU · 5/hr · ~10.0h left");
  expect(second).toBe(first);
  expect(harness.run).toHaveBeenCalledTimes(1);
});

it("shows nothing when colab usage fails", async () => {
  const harness = setup({ initial: {}, sessions: "", upCode: 0 });
  harness.run.mockResolvedValueOnce({ code: 1, stdout: "", stderr: "" });
  await expect(harness.runtime.usageText()).resolves.toBeUndefined();
});
