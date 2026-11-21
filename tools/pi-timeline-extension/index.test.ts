// This TypeScript file is executed with Bun.
import { chmod, mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { afterEach, expect, test, vi } from "vitest";
import timelineExtension from "./index.ts";
import { start, type Harness } from "./src/harness.ts";
import type { Timeline } from "./src/tree.ts";
import { TranslatedTimeline } from "./src/translate.ts";

const directories: string[] = [];

/** One tool call after another, as pi delivers them, since the count of steps is the point. */
async function stepCalls(harness: Harness, times: number, toolName = "read"): Promise<void> {
  if (times === 0) {
    return;
  }
  await harness.handlers.get("tool_call")?.({ toolName }, harness.context);
  await stepCalls(harness, times - 1, toolName);
}

afterEach(async () => {
  await Promise.all(
    directories.splice(0).map(async (dir) => rm(dir, { force: true, recursive: true })),
  );
});

const text = (result: { content: readonly { text: string }[] }): string =>
  result.content.map((part) => part.text).join("\n");

test("the tool records a tree and reports its progress as a status", async () => {
  const harness = await start();
  expect(harness.setStatus).toHaveBeenLastCalledWith("timeline", "0%");
  const added = await harness.run({ action: "add", title: "Fix bug" });
  expect(text(added)).toBe("░░░░░░░░░░ 0.00% · 1 tasks\n░░░░░░   0.00% · t1 Fix bug");
  const done = await harness.run({ action: "update", id: "t1", status: "done" });
  expect(text(done)).toBe("██████████ 100.00% · 1 tasks\n██████ 100.00% ✔ t1 Fix bug");
  expect(harness.setStatus).toHaveBeenLastCalledWith("timeline", "100%");
  expect(text(await harness.run({ action: "list" }))).toBe(
    "██████████ 100.00% · 1 tasks\n██████ 100.00% ✔ t1 Fix bug",
  );
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
  expect(text(result)).toBe(
    "█████░░░░░ 50.00% · 1 tasks\n███░░░  50.00% ▸ t1 Fix the bug — in review",
  );
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
  expect(text(result)).toBe(
    "░░░░░░░░░░ 0.00% · 2 tasks\n░░░░░░   0.00% · t1 Fix bug\n░░░░░░   0.00%   · t2 Read code",
  );
});

test("a tree whose tasks are all done gives the editor its rows back", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  await harness.run({ action: "update", id: "t1", status: "done" });
  expect(harness.setWidget).toHaveBeenLastCalledWith("timeline", undefined, {
    placement: "belowEditor",
  });
  // The finished tree is still there to read.
  expect(text(await harness.run({ action: "list" }))).toBe(
    "██████████ 100.00% · 1 tasks\n██████ 100.00% ✔ t1 Fix bug",
  );
});

test("a finished tree is not sent for translation", async () => {
  const translate = vi.fn(async (timeline: Timeline): Promise<Timeline> => timeline);
  const harness = await start(undefined, { translate } as unknown as TranslatedTimeline);
  await harness.run({ action: "add", title: "Fix bug" });
  expect(translate).toHaveBeenCalledTimes(1);
  await harness.run({ action: "update", id: "t1", status: "done" });
  expect(translate).toHaveBeenCalledTimes(1);
});

test("shutting the session down takes the widget away", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  await harness.handlers.get("session_shutdown")?.({}, harness.context);
  expect(harness.setWidget).toHaveBeenLastCalledWith("timeline", undefined, {
    placement: "belowEditor",
  });
});

test("the head says how long ago the tree last changed", async () => {
  vi.useFakeTimers({ toFake: ["Date"] });
  try {
    vi.setSystemTime(new Date(2026, 7, 26, 11, 0));
    const harness = await start();
    await harness.run({ action: "add", title: "Fix bug" });
    // Under five minutes the age would only widen the head.
    expect(harness.setWidget).toHaveBeenLastCalledWith(
      "timeline",
      ["░░░░░░░░░░ 0.00% · 1 tasks", "░░░░░░   0.00% · t1 Fix bug"],
      { placement: "belowEditor" },
    );
    vi.setSystemTime(new Date(2026, 7, 26, 11, 42));
    await harness.show();
    expect(harness.setWidget).toHaveBeenLastCalledWith(
      "timeline",
      ["░░░░░░░░░░ 0.00% · 1 tasks · updated 42m", "░░░░░░   0.00% · t1 Fix bug"],
      { placement: "belowEditor" },
    );
  } finally {
    vi.useRealTimers();
  }
});

test("the age is repainted as the tree ages, and stops once it is finished", async () => {
  vi.useFakeTimers({ toFake: ["Date", "clearInterval", "setInterval"] });
  try {
    vi.setSystemTime(new Date(2026, 7, 26, 11, 0));
    const harness = await start();
    await harness.run({ action: "add", title: "Fix bug" });
    // The tick lands a minute later, so the head says how old the tree is by then.
    vi.setSystemTime(new Date(2026, 7, 26, 11, 5));
    await vi.advanceTimersByTimeAsync(60_000);
    expect(harness.setWidget).toHaveBeenLastCalledWith(
      "timeline",
      ["░░░░░░░░░░ 0.00% · 1 tasks · updated 6m", "░░░░░░   0.00% · t1 Fix bug"],
      { placement: "belowEditor" },
    );
    await harness.run({ action: "update", id: "t1", status: "done" });
    const painted = harness.setWidget.mock.calls.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(harness.setWidget.mock.calls.length).toBe(painted);
  } finally {
    vi.useRealTimers();
  }
});

test("a long stretch of work earns one checkpoint notice, mid-turn", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  await stepCalls(harness, 23);
  expect(harness.sendMessage).not.toHaveBeenCalled();
  await stepCalls(harness, 1);
  expect(harness.sendMessage).toHaveBeenCalledWith(
    {
      content: expect.stringContaining("Task progress checkpoint: 1 unfinished") as string,
      customType: "timeline-nudge",
      display: false,
    },
    { deliverAs: "steer" },
  );
  // The count restarted, so the next stretch is reported on its own.
  await stepCalls(harness, 23);
  expect(harness.sendMessage).toHaveBeenCalledTimes(1);
  await stepCalls(harness, 1);
  expect(harness.sendMessage).toHaveBeenCalledTimes(2);
  // A timeline call is the checkpoint itself, and its own call is not counted.
  await harness.run({ action: "update", id: "t1", note: "halfway" });
  await stepCalls(harness, 1, "timeline");
  await stepCalls(harness, 23);
  expect(harness.sendMessage).toHaveBeenCalledTimes(2);
});

test("a finished tree is never reported", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  await harness.run({ action: "update", id: "t1", status: "done" });
  await stepCalls(harness, 30);
  expect(harness.sendMessage).not.toHaveBeenCalled();
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
  expect(harness.notify).toHaveBeenLastCalledWith(
    "░░░░░░░░░░ 0.00% · 1 tasks\n░░░░░░   0.00% · t1 Fix bug",
    "info",
  );
  expect(harness.setStatus).toHaveBeenLastCalledWith("timeline", "0%");
});

test("the tree keeps a place of its own under the editor", async () => {
  const harness = await start();
  // An empty session has nothing to show, and must not leave an empty box behind.
  expect(harness.setWidget).toHaveBeenLastCalledWith("timeline", undefined, {
    placement: "belowEditor",
  });
  await harness.run({ action: "add", title: "Fix bug" });
  expect(harness.setWidget).toHaveBeenLastCalledWith(
    "timeline",
    ["░░░░░░░░░░ 0.00% · 1 tasks", "░░░░░░   0.00% · t1 Fix bug"],
    {
      placement: "belowEditor",
    },
  );
});

test("the widget takes its colours from the theme, and tool output stays plain", async () => {
  const harness = await start(undefined, undefined, (token, piece) =>
    piece === "" ? "" : `${token}|${piece}`,
  );
  await harness.run({ action: "add", title: "Fix bug" });
  expect(harness.setWidget).toHaveBeenLastCalledWith(
    "timeline",
    ["dim|░░░░░░░░░░ muted|0.00% · 1 tasks", "dim|░░░░░░ muted|  0.00% · t1 Fix bug"],
    { placement: "belowEditor" },
  );
  const listed = await harness.run({ action: "list" });
  expect(listed.content[0]?.text).toBe("░░░░░░░░░░ 0.00% · 1 tasks\n░░░░░░   0.00% · t1 Fix bug");
});

test("a copy that a reload leaves behind does not take the sidecar away", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-timeline-"));
  directories.push(directory);
  const file = path.join(directory, "session.jsonl");
  const first = await start(file);
  await first.run({ action: "add", title: "Fix bug" });
  // The reloaded copy starts while the previous one is still open, as /reload does.
  const second = await start(file);
  expect(second.notify).not.toHaveBeenCalled();
  expect(second.setStatus).toHaveBeenLastCalledWith("timeline", "0%");
  expect(second.setWidget).toHaveBeenLastCalledWith(
    "timeline",
    ["░░░░░░░░░░ 0.00% · 1 tasks", "░░░░░░   0.00% · t1 Fix bug"],
    {
      placement: "belowEditor",
    },
  );
  await first.handlers.get("session_shutdown")?.({}, first.context);
  const done = await second.run({ action: "update", id: "t1", status: "done" });
  expect(text(done)).toBe("██████████ 100.00% · 1 tasks\n██████ 100.00% ✔ t1 Fix bug");
  await second.handlers.get("session_shutdown")?.({}, second.context);
});

test("a sidecar another pi holds leaves the tree in memory, and it is still shown", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-timeline-"));
  directories.push(directory);
  const file = path.join(directory, "session.jsonl");
  // A fresh lock directory is what proper-lockfile leaves while another process holds the sidecar.
  await mkdir(`${file}.timeline-durable.lock`, { recursive: true });
  const harness = await start(file);
  expect(harness.notify).toHaveBeenCalledWith(
    expect.stringContaining("this session's tree stays in memory"),
    "warning",
  );
  const added = await harness.run({ action: "add", title: "Fix bug" });
  expect(text(added)).toBe("░░░░░░░░░░ 0.00% · 1 tasks\n░░░░░░   0.00% · t1 Fix bug");
  expect(harness.setWidget).toHaveBeenLastCalledWith(
    "timeline",
    ["░░░░░░░░░░ 0.00% · 1 tasks", "░░░░░░   0.00% · t1 Fix bug"],
    {
      placement: "belowEditor",
    },
  );
  await harness.handlers.get("session_shutdown")?.({}, harness.context);
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
  expect(text(await second.run({ action: "list" }))).toBe(
    "██████████ 100.00% · 1 tasks\n██████ 100.00% ✔ t1 Fix bug",
  );
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

test("the tree is painted in the reader's language once the translation lands", async () => {
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => `【${source}】`,
  });
  const harness = await start(undefined, display);
  await harness.run({ action: "add", title: "Fix bug" });
  // The English is on screen at once, so a slow server never leaves the tree blank.
  expect(harness.setWidget).toHaveBeenCalledWith(
    "timeline",
    ["░░░░░░░░░░ 0.00% · 1 tasks", "░░░░░░   0.00% · t1 Fix bug"],
    {
      placement: "belowEditor",
    },
  );
  await vi.waitFor(() => {
    expect(harness.setWidget).toHaveBeenLastCalledWith(
      "timeline",
      ["░░░░░░░░░░ 0.00% · 1 tasks", "░░░░░░   0.00% · t1 【Fix bug】"],
      { placement: "belowEditor" },
    );
  });
});

test("a text is translated once, and only new text reaches the server", async () => {
  const asked: string[] = [];
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => {
      asked.push(source);
      return `【${source}】`;
    },
  });
  const harness = await start(undefined, display);
  await harness.run({ action: "add", title: "Fix bug" });
  await vi.waitFor(() => {
    expect(asked).toStrictEqual(["Fix bug"]);
  });
  await harness.run({ action: "update", id: "t1", note: "in review" });
  await vi.waitFor(() => {
    expect(asked).toStrictEqual(["Fix bug", "in review"]);
  });
});

test("a text the server could not translate is retried, and the English stays meanwhile", async () => {
  let attempts = 0;
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string | null> => {
      attempts += 1;
      return attempts === 1 ? null : `【${source}】`;
    },
  });
  const harness = await start(undefined, display);
  await harness.run({ action: "add", title: "Fix bug" });
  await vi.waitFor(() => {
    expect(attempts).toBe(1);
  });
  expect(harness.setWidget).toHaveBeenLastCalledWith(
    "timeline",
    ["░░░░░░░░░░ 0.00% · 1 tasks", "░░░░░░   0.00% · t1 Fix bug"],
    {
      placement: "belowEditor",
    },
  );
  await harness.run({ action: "update", id: "t1", status: "active" });
  await vi.waitFor(() => {
    expect(harness.setWidget).toHaveBeenLastCalledWith(
      "timeline",
      ["█████░░░░░ 50.00% · 1 tasks", "███░░░  50.00% ▸ t1 【Fix bug】"],
      { placement: "belowEditor" },
    );
  });
});

test("a translation that lands after a newer tree is discarded", async () => {
  const waiting = new Map<string, ((value: string | null) => void)[]>();
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string | null> =>
      new Promise<string | null>((resolve) => {
        waiting.set(source, [...(waiting.get(source) ?? []), resolve]);
      }),
  });
  const harness = await start(undefined, display);
  await harness.run({ action: "add", title: "First" });
  await harness.run({ action: "add", title: "Second" });
  const answer = (source: string, index: number, value: string): void => {
    const queue = waiting.get(source) ?? [];
    queue[index]?.(value);
  };
  // The newer tree's translation lands first, then the older one must not bring it back.
  answer("First", 1, "【First】");
  answer("Second", 0, "【Second】");
  await vi.waitFor(() => {
    expect(harness.setWidget).toHaveBeenLastCalledWith(
      "timeline",
      [
        "░░░░░░░░░░ 0.00% · 2 tasks",
        "░░░░░░   0.00% · t1 【First】",
        "░░░░░░   0.00% · t2 【Second】",
      ],
      { placement: "belowEditor" },
    );
  });
  answer("First", 0, "【First】");
  await vi.waitFor(() => {
    expect(harness.setWidget).toHaveBeenLastCalledWith(
      "timeline",
      [
        "░░░░░░░░░░ 0.00% · 2 tasks",
        "░░░░░░   0.00% · t1 【First】",
        "░░░░░░   0.00% · t2 【Second】",
      ],
      { placement: "belowEditor" },
    );
  });
});

test("a resumed session paints the restored tree in the reader's language", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-timeline-"));
  directories.push(directory);
  const file = path.join(directory, "session.jsonl");
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => `【${source}】`,
  });
  const first = await start(file, display);
  await first.run({ action: "add", title: "Fix bug" });
  await first.handlers.get("session_shutdown")?.({}, first.context);

  // A restart resumes the session file: no tool call is made before the tree is shown again.
  const second = await start(file, display);
  await vi.waitFor(() => {
    expect(second.setWidget).toHaveBeenLastCalledWith(
      "timeline",
      ["░░░░░░░░░░ 0.00% · 1 tasks", "░░░░░░   0.00% · t1 【Fix bug】"],
      { placement: "belowEditor" },
    );
  });
  await second.handlers.get("session_shutdown")?.({}, second.context);
});

test("an extension given no display reads the reader's settings itself", async () => {
  const handlers = new Map<string, (event: unknown, context: ExtensionContext) => Promise<void>>();
  const setWidget = vi.fn();
  timelineExtension({
    on: (name: string, handler: (event: unknown, context: ExtensionContext) => Promise<void>) => {
      handlers.set(name, handler);
    },
    registerCommand: () => undefined,
    registerTool: () => undefined,
  } as unknown as ExtensionAPI);
  const context = {
    sessionManager: { getSessionFile: () => undefined },
    ui: { notify: vi.fn(), setStatus: vi.fn(), setWidget },
  } as unknown as ExtensionContext;
  await handlers.get("session_start")?.({}, context);
  // An empty tree is nothing to draw, and nothing to translate.
  expect(setWidget).toHaveBeenLastCalledWith("timeline", undefined, { placement: "belowEditor" });
  await handlers.get("session_shutdown")?.({}, context);
});
