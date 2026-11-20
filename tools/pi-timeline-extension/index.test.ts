// This TypeScript file is executed with Bun.
import { chmod, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { afterEach, expect, test, vi } from "vitest";
import timelineExtension from "./index.ts";

interface Harness {
  readonly context: ExtensionContext;
  readonly handlers: Map<string, (event: unknown, context: ExtensionContext) => Promise<void>>;
  readonly notify: ReturnType<typeof vi.fn>;
  readonly run: (params: unknown) => Promise<{ content: readonly { text: string }[] }>;
  readonly setStatus: ReturnType<typeof vi.fn>;
  readonly show: () => Promise<void>;
}

const directories: string[] = [];

afterEach(async () => {
  await Promise.all(
    directories.splice(0).map(async (dir) => rm(dir, { force: true, recursive: true })),
  );
});

async function start(file?: string): Promise<Harness> {
  const handlers = new Map<string, (event: unknown, context: ExtensionContext) => Promise<void>>();
  let tool:
    | ((
        id: string,
        params: unknown,
        signal: undefined,
        update: undefined,
        ctx: ExtensionContext,
      ) => Promise<unknown>)
    | undefined;
  let command: ((args: string, context: ExtensionContext) => Promise<void>) | undefined;
  timelineExtension({
    on: (name: string, handler: (event: unknown, context: ExtensionContext) => Promise<void>) => {
      handlers.set(name, handler);
    },
    registerCommand: (_name: string, definition: { handler: typeof command }) => {
      command = definition.handler;
    },
    registerTool: (definition: { execute: typeof tool }) => {
      tool = definition.execute;
    },
  } as unknown as ExtensionAPI);

  const notify = vi.fn();
  const setStatus = vi.fn();
  const context = {
    sessionManager: { getSessionFile: () => file },
    ui: { notify, setStatus },
  } as unknown as ExtensionContext;
  await handlers.get("session_start")?.({}, context);
  return {
    context,
    handlers,
    notify,
    setStatus,
    async run(params: unknown) {
      const result = (await tool?.("call", params, undefined, undefined, context)) as {
        content: readonly { text: string }[];
      };
      return result;
    },
    async show() {
      await command?.("", context);
    },
  };
}

const text = (result: { content: readonly { text: string }[] }): string =>
  result.content.map((part) => part.text).join("\n");

test("the tool records a tree and reports its progress as a status", async () => {
  const harness = await start();
  expect(harness.setStatus).toHaveBeenLastCalledWith("timeline", "0%");
  const added = await harness.run({ action: "add", title: "Fix bug" });
  expect(text(added)).toBe("0% · 1 tasks\n· t1 Fix bug");
  const done = await harness.run({ action: "update", id: "t1", status: "done" });
  expect(text(done)).toBe("100% · 1 tasks\n✔ t1 Fix bug");
  expect(harness.setStatus).toHaveBeenLastCalledWith("timeline", "100%");
  expect(text(await harness.run({ action: "list" }))).toBe("100% · 1 tasks\n✔ t1 Fix bug");
});

test("every field of a change is applied", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  const result = await harness.run({
    action: "update",
    id: "t1",
    note: "in review",
    status: "active",
    title: "Fix the bug",
    weight: 3,
  });
  expect(text(result)).toBe("50% · 1 tasks\n▸ t1 Fix the bug — in review");
});

test("a subtask is added under its parent, with the weight it was given", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  const result = await harness.run({
    action: "add",
    parent: "t1",
    title: "Read code",
    weight: 2,
  });
  expect(text(result)).toBe("0% · 2 tasks\n· t1 Fix bug (0%)\n  · t2 Read code");
});

test("a change without the field it needs is refused", async () => {
  const harness = await start();
  await expect(harness.run({ action: "add" })).rejects.toThrow("action add needs a title.");
  await expect(harness.run({ action: "update", status: "done" })).rejects.toThrow(
    "action update needs an id.",
  );
});

test("the command shows the tree it keeps", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  await harness.show();
  expect(harness.notify).toHaveBeenLastCalledWith("0% · 1 tasks\n· t1 Fix bug", "info");
  expect(harness.setStatus).toHaveBeenLastCalledWith("timeline", "0%");
});

test("a session with a file keeps its tree beside it and finds it again", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-timeline-"));
  directories.push(directory);
  const file = path.join(directory, "session.jsonl");
  const first = await start(file);
  await first.run({ action: "add", title: "Fix bug" });
  await first.run({ action: "update", id: "t1", status: "done" });
  await first.handlers.get("session_shutdown")?.({}, first.context);

  const second = await start(file);
  expect(second.setStatus).toHaveBeenLastCalledWith("timeline", "100%");
  expect(text(await second.run({ action: "list" }))).toBe("100% · 1 tasks\n✔ t1 Fix bug");
  await second.handlers.get("session_shutdown")?.({}, second.context);
});

test("a save that fails is reported to the user", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-timeline-"));
  directories.push(directory);
  const file = path.join(directory, "session.jsonl");
  const harness = await start(file);
  const sidecar = `${file}.timeline-durable`;
  await chmod(sidecar, 0o500);
  await harness.run({ action: "add", title: "Fix bug" });
  await vi.waitFor(() => {
    expect(harness.notify).toHaveBeenCalledWith(
      expect.stringContaining("Timeline storage stopped"),
      "error",
    );
  });
  await chmod(sidecar, 0o700);
  await harness.handlers.get("session_shutdown")?.({}, harness.context);
});
