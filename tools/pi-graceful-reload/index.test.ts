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
  /** The session's entries, when the test needs a branch more detailed than one appended answer. */
  readonly entries?: readonly Entry[];
}

interface Entry {
  readonly id: string;
  readonly parentId: string | null;
  readonly type: string;
  readonly message?: { readonly role: string };
}

/** The branch a run leaves behind: the leaf it started from, then the answer it appended. */
function branch(scenario: Scenario): readonly Entry[] {
  if (scenario.entries !== undefined) {
    return scenario.entries;
  }
  if (scenario.startLeaf === undefined || scenario.currentLeaf === undefined) {
    return [];
  }
  return [
    {
      id: scenario.startLeaf,
      parentId: null,
      type: "message",
      message: { role: "user" },
    },
    {
      id: scenario.currentLeaf,
      parentId: scenario.startLeaf,
      type: "message",
      message: { role: "assistant" },
    },
  ];
}

function isPromptEntry(entry: Entry | undefined): boolean {
  return (
    entry !== undefined &&
    (entry.type === "custom_message" ||
      (entry.type === "message" && entry.message?.role === "user"))
  );
}

interface Fake {
  /** Fire `agent_start`, then run `/graceful-reload` as if the run had added entries. */
  readonly run: () => Promise<void>;
  readonly calls: string[];
}

function fake(scenario: Scenario): Fake {
  const calls: string[] = [];
  let leaf: string | null = scenario.startLeaf ?? null;
  const entries = new Map<string, Entry>(
    branch(scenario).map((entry) => [entry.id, entry] as const),
  );
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
      getEntry: (id: string) => entries.get(id),
    },
    navigateTree: async (targetId: string) => {
      calls.push(`navigate:${targetId}`);
      if (scenario.navigateError !== undefined) {
        // oxlint-disable-next-line typescript/only-throw-error -- a plugin can reject with anything
        throw scenario.navigateError;
      }
      leaf = targetId;
      // pi answers a prompt's text to the editor, and nothing for an answer.
      return isPromptEntry(entries.get(targetId))
        ? { cancelled: false, editorText: "the interrupted prompt" }
        : { cancelled: false };
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

test("the interrupted prompt comes back to the editor instead of vanishing", async () => {
  // pi emits agent_start before it persists the prompt, so the run started at the previous answer.
  const { calls, run } = fake({
    idle: false,
    startLeaf: "answer-1",
    currentLeaf: "answer-2",
    entries: [
      { id: "question-1", parentId: null, type: "message", message: { role: "user" } },
      { id: "answer-1", parentId: "question-1", type: "message", message: { role: "assistant" } },
      { id: "question-2", parentId: "answer-1", type: "message", message: { role: "user" } },
      { id: "answer-2", parentId: "question-2", type: "message", message: { role: "assistant" } },
    ],
  });
  await run();
  // The leaf moves to question-2's parent, so the earlier answer stays in the session.
  expect(calls).toStrictEqual([
    "abort",
    "waitForIdle",
    "navigate:question-2",
    "setEditorText:the interrupted prompt",
    "reload",
  ]);
});

test("a run that appended only output reverts to where it started", async () => {
  const { calls, run } = fake({
    idle: false,
    startLeaf: "answer-1",
    currentLeaf: "result-1",
    entries: [
      { id: "answer-1", parentId: null, type: "message", message: { role: "assistant" } },
      { id: "answer-2", parentId: "answer-1", type: "message", message: { role: "assistant" } },
      { id: "result-1", parentId: "answer-2", type: "toolResult" },
    ],
  });
  await run();
  expect(calls).toStrictEqual(["abort", "waitForIdle", "navigate:answer-1", "reload"]);
});

test("a message an extension injected is dropped without taking the editor", async () => {
  const { calls, run } = fake({
    idle: false,
    startLeaf: "answer-1",
    currentLeaf: "answer-2",
    entries: [
      { id: "answer-1", parentId: null, type: "message", message: { role: "assistant" } },
      { id: "nudge-1", parentId: "answer-1", type: "custom_message" },
      { id: "answer-2", parentId: "nudge-1", type: "message", message: { role: "assistant" } },
    ],
  });
  await run();
  expect(calls).toStrictEqual(["abort", "waitForIdle", "navigate:answer-1", "reload"]);
});

test("a branch that no longer descends from where the run started is left alone", async () => {
  const { calls, run } = fake({
    idle: false,
    startLeaf: "answer-1",
    currentLeaf: "answer-9",
    entries: [{ id: "answer-9", parentId: null, type: "message", message: { role: "assistant" } }],
  });
  await run();
  expect(calls).toStrictEqual(["abort", "waitForIdle", "reload"]);
});

test("an entry the session no longer holds is not navigated to", async () => {
  const { calls, run } = fake({
    idle: false,
    startLeaf: "answer-1",
    currentLeaf: "answer-9",
    entries: [
      { id: "answer-9", parentId: "gone", type: "message", message: { role: "assistant" } },
    ],
  });
  await run();
  expect(calls).toStrictEqual(["abort", "waitForIdle", "reload"]);
});
