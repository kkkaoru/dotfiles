// This TypeScript file is executed with Bun.
import type { InputEvent, MarkdownTransformContext } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import { SECTION_NAME } from "./policy.ts";
import {
  createRuntime,
  displayOf,
  errorMessage,
  hiddenWidget,
  holdHost,
  installRule,
  schedule,
  scheduleReply,
  startSession,
  translateInput,
  translateText,
  type Runtime,
  type UiSink,
} from "./runtime.ts";
import type { Server } from "./server.ts";
import type { PlamoSettings } from "./settings.ts";

const settings: PlamoSettings = { replyLanguage: "ja", promptLanguage: "en", port: 18555 };

function server(
  complete: Server["complete"],
  ensure: Server["ensure"] = () => Promise.resolve(true),
): Server {
  return { complete, ensure };
}

function ui(): UiSink & { readonly notes: string[]; readonly status: Array<string | undefined> } {
  const notes: string[] = [];
  const status: Array<string | undefined> = [];
  return {
    notes,
    status,
    notify: (message) => {
      notes.push(message);
    },
    setStatus: (_key, text) => {
      status.push(text);
    },
    setWidget: () => undefined,
  };
}

// Nothing waits: a test that does not drive the progress clock never sees a report.
const noSleep = (): Promise<void> =>
  new Promise<void>(() => {
    // Never resolves: the progress loop stays parked.
  });

function runtime(complete: Server["complete"], ensure?: Server["ensure"]): Runtime {
  return createRuntime(settings, server(complete, ensure), noSleep);
}

interface Clock {
  readonly waits: number[];
  readonly step: () => Promise<void>;
  readonly sleep: (ms: number) => Promise<void>;
}

// A clock the test advances by hand: each step releases the oldest pending wait.
function manualClock(): Clock {
  const waits: number[] = [];
  const waiting: (() => void)[] = [];
  return {
    waits,
    sleep: (ms) => {
      waits.push(ms);
      return new Promise<void>((resolve) => {
        waiting.push(resolve);
      });
    },
    step: async () => {
      const release = waiting.shift();
      release?.();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    },
  };
}

function input(text: string, source: InputEvent["source"] = "interactive"): InputEvent {
  return { type: "input", text, source };
}

const shown: MarkdownTransformContext = {
  messageType: "assistant",
  isStreaming: false,
  availableWidth: 40,
};

test("an error message falls back when the failure is not an Error", () => {
  expect(errorMessage(new Error("disk"))).toBe("disk");
  expect(errorMessage("disk")).toBe("unknown error");
});

test("a Japanese prompt becomes English and is remembered for display", async () => {
  const sink = ui();
  const result = await translateInput(
    runtime(async () => "Add a tab"),
    input("タブを追加して"),
    sink,
  );
  expect(result).toStrictEqual({ action: "transform", text: "Add a tab" });
  expect(sink.status).toStrictEqual(["PLaMo: 翻訳中… 0/1", undefined]);
  expect(
    displayOf(
      runtime(async () => "Add a tab"),
      "Add a tab",
      {
        messageType: "user",
        isStreaming: false,
        availableWidth: 80,
      },
    ),
  ).toBe("Add a tab");
});

test("the remembered Japanese is what the transcript shows", async () => {
  const box = runtime(async () => "Add a tab");
  await translateInput(box, input("タブを追加して"), ui());
  expect(
    displayOf(box, "Add a tab", { messageType: "user", isStreaming: false, availableWidth: 80 }),
  ).toBe("タブを追加して");
});

test("English, commands, and extension prompts pass through", async () => {
  const box = runtime(async () => "SHOULD NOT RUN");
  expect(await translateInput(box, input("add a tab"), ui())).toStrictEqual({ action: "continue" });
  expect(await translateInput(box, input("/goal 調べる"), ui())).toStrictEqual({
    action: "continue",
  });
  expect(await translateInput(box, input("タブ", "extension"), ui())).toStrictEqual({
    action: "continue",
  });
});

test("images stay attached to the translated prompt", async () => {
  const event = input("タブを追加して");
  event.images = [{ type: "image", data: "abc", mimeType: "image/png" }];
  expect(
    await translateInput(
      runtime(async () => "Add a tab"),
      event,
      ui(),
    ),
  ).toStrictEqual({
    action: "transform",
    text: "Add a tab",
    images: [{ type: "image", data: "abc", mimeType: "image/png" }],
  });
});

test("an unreachable server leaves the prompt as typed and warns once", async () => {
  const sink = ui();
  const box = runtime(
    async () => "x",
    () => Promise.resolve(false),
  );
  expect(await translateInput(box, input("タブを追加して"), sink)).toStrictEqual({
    action: "continue",
  });
  expect(await translateInput(box, input("もう一つ"), sink)).toStrictEqual({ action: "continue" });
  expect(sink.notes).toStrictEqual([
    "plamo-translate: the PLaMo server is not reachable; run scripts/setup-plamo-translate.sh. Text is passed through untranslated.",
  ]);
});

test("a failed request keeps the source and warns once", async () => {
  const sink = ui();
  const box = runtime(async () => {
    throw new Error("boom");
  });
  box.ui = sink;
  expect(await translateText(box, "Add a tab", "ja")).toBe("Add a tab");
  expect(await translateText(box, "Add a pane", "ja")).toBe("Add a pane");
  expect(sink.notes).toStrictEqual(["plamo-translate: a translation request failed: boom"]);
});

test("a dropped connection is retried once", async () => {
  const calls = { count: 0 };
  const box = runtime(async (text) => {
    calls.count += 1;
    if (calls.count === 1) {
      throw new Error("ECONNRESET");
    }
    return `EN:${text}`;
  });
  expect(await translateText(box, "タブ", "en")).toBe("EN:タブ");
  expect(calls.count).toBe(2);
  expect(box.warned).toBe(false);
});

test("display waits out a stream and then uses a finished translation", async () => {
  const box = runtime(async (text) => `JA:${text}`);
  expect(
    displayOf(box, "Add a tab", {
      messageType: "assistant",
      isStreaming: true,
      availableWidth: 40,
    }),
  ).toBe("Add a tab");
  expect(displayOf(box, "Add a tab", shown)).toBe("Add a tab");
  await box.pending.get("Add a tab");
  expect(displayOf(box, "Add a tab", shown)).toBe("JA:Add a tab");
});

test("a second request for the same text does not translate again", async () => {
  const calls = { count: 0 };
  const box = runtime(async (text) => {
    calls.count += 1;
    return `JA:${text}`;
  });
  schedule(box, "Add a tab", "ja");
  schedule(box, "Add a tab", "ja");
  await box.pending.get("Add a tab");
  schedule(box, "Add a tab", "ja");
  expect(calls.count).toBe(1);
  expect(box.ready.get("Add a tab")).toBe("JA:Add a tab");
});

test("two translations keep the status until both finish", async () => {
  const sink = ui();
  const box = runtime(async (text) => `JA:${text}`);
  box.ui = sink;
  schedule(box, "one", "ja");
  schedule(box, "two", "ja");
  await box.pending.get("one");
  await box.pending.get("two");
  expect(sink.status).toStrictEqual([
    "PLaMo: 翻訳中… 0/1",
    "PLaMo: 翻訳中… 0/2",
    "PLaMo: 翻訳中… 1/2",
    undefined,
  ]);
});

test("a settled translation redraws the transcript", async () => {
  const host = {
    invalidated: 0,
    rendered: 0,
    invalidate(): void {
      this.invalidated += 1;
    },
    requestRender(): void {
      this.rendered += 1;
    },
  };
  const box = runtime(async (text) => `JA:${text}`);
  holdHost(box, host);
  schedule(box, "Add a tab", "ja");
  await box.pending.get("Add a tab");
  expect(host.invalidated).toBe(1);
  expect(host.rendered).toBe(1);
});

test("a slow translation reports progress on one chat line", async () => {
  const sink = ui();
  const clock = manualClock();
  const deferred: { release: (value: string) => void } = { release: () => undefined };
  const box = createRuntime(
    settings,
    server(
      async () =>
        new Promise<string>((resolve) => {
          deferred.release = resolve;
        }),
    ),
    clock.sleep,
  );
  box.ui = sink;
  schedule(box, "Add a tab", "ja");
  await clock.step();
  expect(sink.notes).toStrictEqual(["PLaMo: 翻訳中… 0/1"]);
  deferred.release("JA:Add a tab");
  await box.pending.get("Add a tab");
  expect(sink.notes).toStrictEqual(["PLaMo: 翻訳中… 0/1", ""]);
});

test("a translation that finishes in time shows no progress line", async () => {
  const sink = ui();
  const clock = manualClock();
  const box = createRuntime(
    settings,
    server(async (text) => `JA:${text}`),
    clock.sleep,
  );
  box.ui = sink;
  schedule(box, "Add a tab", "ja");
  await box.pending.get("Add a tab");
  await clock.step();
  expect(sink.notes).toStrictEqual([]);
});

test("a second slow translation reuses the progress line", async () => {
  const sink = ui();
  const clock = manualClock();
  const finished: string[] = [];
  const box = createRuntime(
    settings,
    server(
      async (text) =>
        new Promise<string>((resolve) => {
          finished.push(text);
          if (finished.length === 2) {
            resolve(`JA:${text}`);
          }
        }),
    ),
    clock.sleep,
  );
  box.ui = sink;
  schedule(box, "one", "ja");
  await clock.step();
  expect(sink.notes).toStrictEqual(["PLaMo: 翻訳中… 0/1"]);
  expect(box.progress.looping).toBe(true);
  expect(clock.waits).toStrictEqual([2000, 5000]);
});

test("progress keeps reporting while a run continues and stops when it ends", async () => {
  const sink = ui();
  const clock = manualClock();
  const box = createRuntime(
    settings,
    server(
      async () =>
        new Promise<string>(() => {
          // Never finishes on its own.
        }),
    ),
    clock.sleep,
  );
  box.ui = sink;
  schedule(box, "one", "ja");
  await clock.step();
  await clock.step();
  expect(sink.notes).toStrictEqual(["PLaMo: 翻訳中… 0/1", "PLaMo: 翻訳中… 0/1"]);
  box.progress = { ...box.progress, done: 1, total: 0 };
  await clock.step();
  expect(sink.notes).toStrictEqual(["PLaMo: 翻訳中… 0/1", "PLaMo: 翻訳中… 0/1", ""]);
  expect(box.progress.looping).toBe(false);
});

test("progress updates in place when one of two requests finishes", async () => {
  const sink = ui();
  const clock = manualClock();
  const box = createRuntime(
    settings,
    server(async (text) =>
      text === "one"
        ? `JA:${text}`
        : new Promise<string>(() => {
            // Never finishes.
          }),
    ),
    clock.sleep,
  );
  box.ui = sink;
  schedule(box, "one", "ja");
  schedule(box, "two", "ja");
  await box.pending.get("one");
  await clock.step();
  expect(sink.notes).toStrictEqual(["PLaMo: 翻訳中… 1/2"]);
  await box.pending.get("one");
  expect(box.progress.shown).toBe(true);
});

test("a failure while showing progress is reported once", async () => {
  const sink = ui();
  const box = createRuntime(
    settings,
    server(async () => "x"),
    () => Promise.reject(new Error("clock")),
  );
  box.ui = sink;
  schedule(box, "one", "ja");
  await Promise.resolve();
  await Promise.resolve();
  expect(sink.notes).toStrictEqual(["plamo-translate: showing progress failed: clock"]);
});

test("reply language off skips display translation", () => {
  const box = createRuntime(
    { replyLanguage: "off", promptLanguage: "en", port: 18555 },
    server(async () => "x"),
  );
  expect(displayOf(box, "Add a tab", shown)).toBe("Add a tab");
  expect(box.pending.size).toBe(0);
});

test("the prompt rule is installed unless both directions are off", () => {
  const sections: Record<string, string> = {};
  installRule(
    runtime(async () => "x"),
    sections,
  );
  expect(sections[SECTION_NAME]).toBe(
    "The user writes in Japanese. Their messages are machine-translated into English before you read them, and your replies are machine-translated into `ja` for display only. Read and write English. Only inline code and fenced code blocks are shown exactly as written. Put anything the user must read or copy verbatim in one of them: commands, file paths, identifiers, option and flag names, literal values, error messages and quoted UI text. Do not wrap ordinary prose.",
  );
  const empty: Record<string, string> = {};
  installRule(
    createRuntime(
      { replyLanguage: "off", promptLanguage: "off", port: 1 },
      server(async () => "x"),
    ),
    empty,
  );
  expect(empty).toStrictEqual({});
});

test("a finished assistant reply is queued for display", async () => {
  const box = runtime(async (text) => `JA:${text}`);
  scheduleReply(box, { role: "assistant", content: [{ type: "text", text: " Add a tab " }] });
  await box.pending.get("Add a tab");
  expect(box.ready.get("Add a tab")).toBe("JA:Add a tab");
});

test("reply translation off ignores assistant messages", () => {
  const box = createRuntime(
    { replyLanguage: "off", promptLanguage: "en", port: 1 },
    server(async () => "x"),
  );
  scheduleReply(box, { role: "assistant", content: [{ type: "text", text: "Add a tab" }] });
  expect(box.pending.size).toBe(0);
});

test("session start captures the renderer and warms the server", async () => {
  const warmed = { count: 0 };
  const box = runtime(
    async () => "x",
    () => {
      warmed.count += 1;
      return Promise.resolve(true);
    },
  );
  const sink = ui();
  const hosts: object[] = [];
  sink.setWidget = (_key, factory) => {
    hosts.push(
      factory({
        invalidate(): void {},
        requestRender(): void {},
      }),
    );
  };
  startSession(box, sink);
  await Promise.resolve();
  expect(warmed.count).toBe(1);
  expect(hosts).toHaveLength(1);
  expect(box.host).not.toBeNull();
});

test("status calls before a session are ignored", () => {
  const box = createRuntime(
    settings,
    server(async () => "x"),
  );
  box.ui.notify("hi", "warning");
  box.ui.setStatus("plamo-translate", undefined);
  box.ui.setWidget("plamo-translate", () => hiddenWidget(), { placement: "belowEditor" });
  expect(box.warned).toBe(false);
});

test("the hidden widget draws nothing", () => {
  const widget = hiddenWidget();
  expect(widget.render(10)).toStrictEqual([]);
  widget.invalidate?.();
});

test("a warm-up failure is reported", async () => {
  const sink = ui();
  const box = runtime(
    async () => "x",
    () => Promise.reject(new Error("down")),
  );
  startSession(box, sink);
  await Promise.resolve();
  await Promise.resolve();
  expect(sink.notes).toStrictEqual(["plamo-translate: warming the server failed: down"]);
});

test("a translation that rejects while settling is reported", async () => {
  const sink = ui();
  const box = runtime(async () => "kept");
  box.ui = sink;
  box.ready.set = () => {
    throw new Error("settle");
  };
  schedule(box, "Add a tab", "ja");
  await box.pending.get("Add a tab")?.catch(() => undefined);
  expect(sink.notes).toStrictEqual(["plamo-translate: settling a translation failed: settle"]);
});
