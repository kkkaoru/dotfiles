// This TypeScript file is executed with Bun.
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";
import { expect, test, vi } from "vitest";
import gracefulReload from "./index.ts";

interface Scenario {
  readonly idle: boolean;
  readonly settles?: boolean;
  readonly reloadError?: unknown;
  readonly navigateError?: unknown;
  /** The leaf the run started from, and where the session stands when the command runs. */
  readonly startLeaf?: string;
  readonly currentLeaf?: string;
  readonly editor?: string;
}

interface Fake {
  /** Fire `agent_start`, then run `/graceful-reload` as if the run had added entries. */
  readonly run: () => Promise<void>;
  readonly calls: string[];
}

function fake(scenario: Scenario): Fake {
  const calls: string[] = [];
  let leaf: string | null = scenario.startLeaf ?? null;
  const handlers = new Map<string, (event: unknown, ctx: ExtensionCommandContext) => void>();
  const ctx = {
    // A run that ignores the abort stays live, so the session never becomes idle.
    isIdle: () => scenario.idle || (calls.includes("waitForIdle") && scenario.settles !== false),
    abort: () => {
      calls.push("abort");
    },
    waitForIdle: async () => {
      calls.push("waitForIdle");
      if (scenario.settles === false) {
        await new Promise<void>(() => {});
      }
    },
    sessionManager: {
      getLeafId: () => leaf,
    },
    navigateTree: async (targetId: string) => {
      calls.push(`navigate:${targetId}`);
      if (scenario.navigateError !== undefined) {
        // oxlint-disable-next-line typescript/only-throw-error -- a plugin can reject with anything
        throw scenario.navigateError;
      }
      leaf = targetId;
      return { cancelled: false, editorText: "the interrupted prompt" };
    },
    reload: async () => {
      calls.push("reload");
      if (scenario.reloadError !== undefined) {
        // oxlint-disable-next-line typescript/only-throw-error -- a plugin can reject with anything
        throw scenario.reloadError;
      }
    },
    ui: {
      notify: (message: string, type?: string) => {
        calls.push(`notify:${String(type)}:${message}`);
      },
      getEditorText: () => scenario.editor ?? "",
      setEditorText: (text: string) => {
        calls.push(`setEditorText:${text}`);
      },
    },
  } as unknown as ExtensionCommandContext;
  const commands = new Map<string, (args: string, ctx: ExtensionCommandContext) => Promise<void>>();
  gracefulReload({
    on: (event: string, handler: (event: unknown, ctx: ExtensionCommandContext) => void) => {
      handlers.set(event, handler);
      return () => {
        handlers.delete(event);
      };
    },
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
  return {
    calls,
    run: async () => {
      const start = leaf;
      handlers.get("agent_start")?.({ type: "agent_start" }, ctx);
      leaf = scenario.currentLeaf ?? start;
      await handler("", ctx);
    },
  };
}

test("aborts, reverts the interrupted turn and reloads", async () => {
  const { calls, run } = fake({ idle: false, startLeaf: "user-1", currentLeaf: "assistant-2" });
  await run();
  expect(calls).toStrictEqual([
    "abort",
    "waitForIdle",
    "navigate:user-1",
    "setEditorText:the interrupted prompt",
    "reload",
  ]);
});

test("reloads without aborting when nothing is running", async () => {
  const { calls, run } = fake({ idle: true, startLeaf: "user-1" });
  await run();
  expect(calls).toStrictEqual(["reload"]);
});

test("keeps what is already in the editor", async () => {
  const { calls, run } = fake({
    idle: false,
    startLeaf: "user-1",
    currentLeaf: "assistant-2",
    editor: "typed already",
  });
  await run();
  expect(calls).toStrictEqual(["abort", "waitForIdle", "navigate:user-1", "reload"]);
});

test("does not navigate when the aborted run added nothing", async () => {
  const { calls, run } = fake({ idle: false, startLeaf: "user-1" });
  await run();
  expect(calls).toStrictEqual(["abort", "waitForIdle", "reload"]);
});

test("reloads even when a tool ignores the abort, without navigating", async () => {
  vi.useFakeTimers();
  const { calls, run } = fake({
    idle: false,
    settles: false,
    startLeaf: "user-1",
    currentLeaf: "assistant-2",
  });
  const done = run();
  await vi.advanceTimersByTimeAsync(3000);
  await done;
  vi.useRealTimers();
  expect(calls).toStrictEqual(["abort", "waitForIdle", "reload"]);
});

test("reports a navigation pi refuses, then reloads", async () => {
  const { calls, run } = fake({
    idle: false,
    startLeaf: "user-1",
    currentLeaf: "assistant-2",
    navigateError: new Error("still streaming"),
  });
  await run();
  expect(calls).toStrictEqual([
    "abort",
    "waitForIdle",
    "navigate:user-1",
    "notify:warning:graceful-reload: could not revert the interrupted turn: still streaming",
    "reload",
  ]);
});

test("reports a reload pi still refuses", async () => {
  const { calls, run } = fake({
    idle: false,
    startLeaf: "user-1",
    currentLeaf: "assistant-2",
    reloadError: new Error("still streaming"),
  });
  await run();
  expect(calls).toStrictEqual([
    "abort",
    "waitForIdle",
    "navigate:user-1",
    "setEditorText:the interrupted prompt",
    "reload",
    "notify:error:graceful-reload: still streaming",
  ]);
});

test("reports a rejection that is not an Error", async () => {
  const { calls, run } = fake({ idle: false, reloadError: "still streaming" });
  await run();
  expect(calls.at(-1)).toBe("notify:error:graceful-reload: still streaming");
});
