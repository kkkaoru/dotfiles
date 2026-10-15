import { expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import colabLifecycleExtension, {
  applyOutcome,
  createColabExtension,
  createDeps,
  parseState,
  type ColabContext,
  type ColabLifecycleHost,
} from "./index.ts";
import type { Deps } from "./src/runtime.ts";

type Handler = Parameters<ColabLifecycleHost["on"]>[1];

function context(provider: string | undefined): ColabContext {
  return {
    model: provider === undefined ? undefined : { provider },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
}

it("reads url and lastRequestAt", () => {
  expect(parseState('{"url":"u","lastRequestAt":5}')).toStrictEqual({ url: "u", lastRequestAt: 5 });
});

it("ignores malformed or mistyped state", () => {
  expect(parseState("nope")).toStrictEqual({});
  expect(parseState("null")).toStrictEqual({});
  expect(parseState('{"url":1,"lastRequestAt":"x"}')).toStrictEqual({});
});

it("points the provider at the new URL", () => {
  const registerProvider = vi.fn();
  applyOutcome({ registerProvider }, context("google-colab"), {
    ok: true,
    started: false,
    url: "https://x",
  });
  expect(registerProvider).toHaveBeenCalledWith("google-colab", { baseUrl: "https://x/v1" });
});

it("warns instead of throwing when Colab is unavailable", () => {
  const ctx = context("google-colab");
  applyOutcome({ registerProvider: vi.fn() }, ctx, { ok: false, started: true, reason: "boom" });
  expect(ctx.ui.notify).toHaveBeenCalledWith("google-colab unavailable: boom", "warning");
  applyOutcome({ registerProvider: vi.fn() }, ctx, { ok: false, started: true });
  expect(ctx.ui.notify).toHaveBeenCalledWith("google-colab unavailable: unknown error", "warning");
});

it("registers the lifecycle and activity handlers", () => {
  const events: string[] = [];
  colabLifecycleExtension({
    exec: vi.fn(),
    on: (event) => {
      events.push(event);
    },
    registerProvider: vi.fn(),
  });
  expect(events).toStrictEqual([
    "before_agent_start",
    "model_select",
    "before_provider_request",
    "tool_execution_end",
    "agent_end",
    "agent_end",
  ]);
});

it("ignores other providers on every handler", async () => {
  const handlers = new Map<string, Handler>();
  const exec = vi.fn();
  colabLifecycleExtension({
    exec,
    on: (event, handler) => {
      handlers.set(event, handler);
    },
    registerProvider: vi.fn(),
  });
  await handlers.get("before_agent_start")?.({}, context("omlx"));
  await handlers.get("model_select")?.({ model: { provider: "omlx" } }, context("omlx"));
  await handlers.get("model_select")?.({}, context("omlx"));
  await handlers.get("agent_end")?.({}, context("omlx"));
  expect(exec).not.toHaveBeenCalled();
});

vi.mock("node:fs", () => ({
  mkdirSync: vi.fn(),
  openSync: vi.fn(() => 3),
  realpathSync: vi.fn((value: string) => value),
  readFileSync: vi.fn(() => '{"url":"https://s.trycloudflare.com","lastRequestAt":7}'),
  writeFileSync: vi.fn(),
}));
vi.mock("node:child_process", () => ({
  spawn: vi.fn(() => ({
    on: vi.fn((_event: string, callback: () => void) => {
      callback();
    }),
    unref: vi.fn(),
  })),
}));

const testConfig = {
  colabCommand: "colab",
  upScript: "up.sh",
  upTimeoutMs: 1,
  watcherScript: "watch.sh",
};

function harness(deps: Deps): {
  handlers: Map<string, Handler>;
  registerProvider: ReturnType<typeof vi.fn>;
} {
  const handlers = new Map<string, Handler>();
  const registerProvider = vi.fn();
  createColabExtension(
    {
      exec: vi.fn(),
      on: (event, handler) => {
        handlers.set(event, handler);
      },
      registerProvider,
    },
    deps,
    testConfig,
  );
  return { handlers, registerProvider };
}

function liveDeps(): Deps {
  return {
    run: vi.fn(async () => ({ code: 0, stdout: "[qwen]", stderr: "" })),
    spawnDetached: vi.fn(),
    readState: () => ({ url: "https://live.trycloudflare.com" }),
    writeState: vi.fn(),
    now: () => 9,
  };
}

it("points the provider at Colab before an agent run on google-colab", async () => {
  const { handlers, registerProvider } = harness(liveDeps());
  await handlers.get("before_agent_start")?.({}, context("google-colab"));
  expect(registerProvider).toHaveBeenCalledWith("google-colab", {
    baseUrl: "https://live.trycloudflare.com/v1",
  });
});

it("pre-warms Colab when switching into google-colab", async () => {
  const { handlers, registerProvider } = harness(liveDeps());
  await handlers.get("model_select")?.(
    { model: { provider: "google-colab" } },
    context("google-colab"),
  );
  await vi.waitFor(() => {
    expect(registerProvider).toHaveBeenCalledTimes(1);
  });
});

it("records activity only for google-colab requests", async () => {
  const deps = liveDeps();
  const { handlers } = harness(deps);
  await handlers.get("before_provider_request")?.({}, context("google-colab"));
  await handlers.get("agent_end")?.({}, context("omlx"));
  expect(deps.writeState).toHaveBeenCalledTimes(1);
});

it("swallows a failed pre-warm", async () => {
  const deps: Deps = { ...liveDeps(), run: vi.fn().mockRejectedValue(new Error("down")) };
  const { handlers, registerProvider } = harness(deps);
  await handlers.get("model_select")?.(
    { model: { provider: "google-colab" } },
    context("google-colab"),
  );
  await vi.waitFor(() => {
    expect(deps.run).toHaveBeenCalled();
  });
  expect(registerProvider).not.toHaveBeenCalled();
});

it("wires real state and process adapters", async () => {
  const deps = createDeps(
    { exec: vi.fn(async () => ({ code: 0, stdout: "ok", stderr: "" })) },
    "/tmp/state.json",
  );
  expect(deps.readState()).toStrictEqual({ url: "https://s.trycloudflare.com", lastRequestAt: 7 });
  deps.writeState({ url: "u" });
  deps.spawnDetached("watch.sh", []);
  await expect(deps.run("colab", ["sessions"], 5)).resolves.toStrictEqual({
    code: 0,
    stdout: "ok",
    stderr: "",
  });
  expect(deps.now()).toBeGreaterThan(0);
});

it("falls back to empty state when the file is unreadable", () => {
  vi.mocked(readFileSync).mockImplementationOnce(() => {
    throw new Error("ENOENT");
  });
  const deps = createDeps({ exec: vi.fn() }, "/tmp/missing.json");
  expect(deps.readState()).toStrictEqual({});
});

it("shows the Colab balance in the footer while a google-colab model is active", async () => {
  const deps: Deps = {
    ...liveDeps(),
    run: vi.fn(async () => ({
      code: 0,
      stdout: "[qwen]\nCurrent balance: 194.09 compute units\nUsage rate: 5.38/hr",
      stderr: "",
    })),
  };
  const { handlers } = harness(deps);
  const ctx = context("google-colab");
  await handlers.get("agent_end")?.({}, ctx);
  expect(ctx.ui.setStatus).toHaveBeenCalledWith(
    "google-colab",
    "Colab 194.1 CU · 5.38/hr · ~36.1h left",
  );
});

it("clears the footer balance for other models", async () => {
  const { handlers } = harness(liveDeps());
  const ctx = context("omlx");
  await handlers.get("agent_end")?.({}, ctx);
  await handlers.get("model_select")?.({ model: { provider: "omlx" } }, ctx);
  expect(ctx.ui.setStatus).toHaveBeenCalledWith("google-colab", undefined);
});
