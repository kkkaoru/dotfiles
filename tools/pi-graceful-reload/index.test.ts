// This TypeScript file is executed with Bun.
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { expect, test, vi } from "vitest";
import gracefulReload from "./index.ts";

interface Fake {
  readonly ctx: ExtensionCommandContext;
  readonly calls: string[];
}

function fake(options: { idle: boolean; settles?: boolean; reloadError?: unknown }): Fake {
  const calls: string[] = [];
  const ctx = {
    isIdle: () => options.idle,
    abort: () => {
      calls.push("abort");
    },
    waitForIdle: async () => {
      calls.push("waitForIdle");
      if (options.settles === false) {
        await new Promise<void>(() => {});
      }
    },
    reload: async () => {
      calls.push("reload");
      if (options.reloadError !== undefined) {
        // oxlint-disable-next-line typescript/only-throw-error -- a plugin can reject with anything
        throw options.reloadError;
      }
    },
    ui: {
      notify: (message: string, type?: string) => {
        calls.push(`notify:${String(type)}:${message}`);
      },
    },
  } as unknown as ExtensionCommandContext;
  return { calls, ctx };
}

function register(): (args: string, ctx: ExtensionCommandContext) => Promise<void> {
  const commands = new Map<string, (args: string, ctx: ExtensionCommandContext) => Promise<void>>();
  gracefulReload({
    registerCommand: (
      name: string,
      options: { handler: (args: string, ctx: ExtensionCommandContext) => Promise<void> },
    ) => {
      commands.set(name, options.handler);
    },
  } as unknown as ExtensionAPI);
  const handler = commands.get("graceful-reload");
  if (handler === undefined) {
    throw new Error("graceful-reload was not registered");
  }
  return handler;
}

test("aborts a running response, waits for it and reloads", async () => {
  const { calls, ctx } = fake({ idle: false });
  await register()("", ctx);
  expect(calls).toStrictEqual(["abort", "waitForIdle", "reload"]);
});

test("reloads without aborting when nothing is running", async () => {
  const { calls, ctx } = fake({ idle: true });
  await register()("", ctx);
  expect(calls).toStrictEqual(["reload"]);
});

test("reloads even when a tool ignores the abort", async () => {
  vi.useFakeTimers();
  const { calls, ctx } = fake({ idle: false, settles: false });
  const done = register()("", ctx);
  await vi.advanceTimersByTimeAsync(3000);
  await done;
  vi.useRealTimers();
  expect(calls).toStrictEqual(["abort", "waitForIdle", "reload"]);
});

test("reports a reload pi still refuses", async () => {
  const { calls, ctx } = fake({ idle: false, reloadError: new Error("still streaming") });
  await register()("", ctx);
  expect(calls).toStrictEqual([
    "abort",
    "waitForIdle",
    "reload",
    "notify:error:graceful-reload: still streaming",
  ]);
});

test("reports a rejection that is not an Error", async () => {
  const { calls, ctx } = fake({ idle: false, reloadError: "still streaming" });
  await register()("", ctx);
  expect(calls.at(-1)).toBe("notify:error:graceful-reload: still streaming");
});
