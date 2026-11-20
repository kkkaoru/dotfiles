// This TypeScript file is executed with Bun.
import { chmod, mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { lock } from "proper-lockfile";
import { afterEach, expect, test, vi } from "vitest";
import promptCacheExtension from "./index.ts";

interface Harness {
  readonly notify: ReturnType<typeof vi.fn>;
  readonly run: (name: string, event: unknown) => Promise<void>;
  readonly setStatus: ReturnType<typeof vi.fn>;
  readonly command: (args: string) => Promise<void>;
}

const directories: string[] = [];

afterEach(async () => {
  await Promise.all(
    directories.splice(0).map(async (dir) => rm(dir, { force: true, recursive: true })),
  );
});

async function start(file?: string): Promise<Harness> {
  const handlers = new Map<string, (event: unknown, context: ExtensionContext) => Promise<void>>();
  let command: ((args: string, context: ExtensionContext) => Promise<void>) | undefined;
  promptCacheExtension({
    on: (name: string, handler: (event: unknown, context: ExtensionContext) => Promise<void>) => {
      handlers.set(name, handler);
    },
    registerCommand: (_name: string, definition: { handler: typeof command }) => {
      command = definition.handler;
    },
  } as unknown as ExtensionAPI);

  const notify = vi.fn();
  const setStatus = vi.fn();
  const context = {
    sessionManager: { getSessionFile: () => file },
    ui: { notify, setStatus },
  } as unknown as ExtensionContext;
  const run = async (name: string, event: unknown): Promise<void> => {
    await handlers.get(name)?.(event, context);
  };
  await run("session_start", {});
  return {
    command: async (args) => {
      await command?.(args, context);
    },
    notify,
    run,
    setStatus,
  };
}

const request = (messages: readonly unknown[]): { payload: unknown } => ({
  payload: { messages, tools: [{ name: "bash" }] },
});

const answer = (input: number, cacheRead: number): unknown => ({
  message: { model: "m", role: "assistant", usage: { cacheRead, input } },
});

test("the footer shows the hit rate of the requests the provider answered", async () => {
  const box = await start();
  await box.run("before_provider_request", request([{ role: "user", content: "a" }]));
  await box.run("message_end", answer(100, 900));
  expect(box.setStatus).toHaveBeenLastCalledWith("prompt-cache", "cache 90%");
});

test("a recomputed prompt names the part that changed", async () => {
  const box = await start();
  await box.run("before_provider_request", request([{ role: "user", content: "a" }]));
  await box.run("message_end", answer(100, 900));
  await box.run(
    "before_provider_request",
    request([
      { role: "user", content: "b" },
      { role: "user", content: "c" },
    ]),
  );
  await box.run("message_end", answer(20_000, 400));
  expect(box.notify).toHaveBeenCalledWith(
    "Cache break: 20400 tokens recomputed after the messages changed.",
    "warning",
  );
});

test("a provider that drops an unchanged prefix is reported once", async () => {
  const box = await start();
  await box.run("before_provider_request", request([{ role: "user", content: "a" }]));
  await box.run("message_end", answer(100, 900));
  await box.run(
    "before_provider_request",
    request([
      { role: "user", content: "a" },
      { role: "user", content: "b" },
    ]),
  );
  await box.run("message_end", answer(20_000, 0));
  await box.run("message_end", answer(20_000, 0));
  expect(box.notify).toHaveBeenCalledTimes(1);
  expect(box.notify).toHaveBeenCalledWith(
    "Cache miss: 20000 tokens recomputed although the prefix was unchanged; the provider dropped it.",
    "warning",
  );
});

test("a small prompt, a user message and a reply without usage are ignored", async () => {
  const box = await start();
  await box.run("before_provider_request", request([{ role: "user", content: "a" }]));
  await box.run("message_end", { message: { role: "user", usage: { input: 20_000 } } });
  await box.run("message_end", { message: { role: "assistant" } });
  await box.run("message_end", answer(100, 0));
  expect(box.notify).not.toHaveBeenCalled();
  expect(box.setStatus).toHaveBeenLastCalledWith("prompt-cache", "cache 0%");
});

test("the ledger survives a reload of the same session", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-prompt-cache-"));
  directories.push(directory);
  const file = path.join(directory, "session.jsonl");

  const first = await start(file);
  await first.run("before_provider_request", request([{ role: "user", content: "a" }]));
  await first.run("message_end", answer(100, 900));
  await first.run("session_shutdown", {});

  const second = await start(file);
  await second.command("");
  expect(second.notify).toHaveBeenCalledWith(
    "90% cached · 1 requests · 1,000 prompt, 900 cached\nNo large prompt was recomputed.",
    "info",
  );
});

test("a save that fails is reported to the user", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-prompt-cache-"));
  directories.push(directory);
  const file = path.join(directory, "session.jsonl");
  const box = await start(file);
  await chmod(`${file}.cache-durable`, 0o500);
  await box.run("message_end", answer(100, 900));
  await vi.waitFor(() => {
    expect(box.notify).toHaveBeenCalledWith(
      expect.stringContaining("Prompt cache storage stopped"),
      "error",
    );
  });
  await chmod(`${file}.cache-durable`, 0o700);
  await box.run("session_shutdown", {});
});

test("a reply without a model or a cache write is still measured", async () => {
  const box = await start();
  await box.run("message_end", { message: { role: "assistant", usage: { input: 10 } } });
  expect(box.setStatus).toHaveBeenLastCalledWith("prompt-cache", "cache 0%");
});

test("a session without a file keeps its ledger in memory", async () => {
  const box = await start();
  await box.command("");
  expect(box.notify).toHaveBeenCalledWith("No request recorded yet.", "info");
});

test("a ledger another pi holds is kept in memory instead of failing every request", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-prompt-cache-"));
  directories.push(directory);
  const file = path.join(directory, "session.jsonl");
  await mkdir(`${file}.cache-durable`, { recursive: true });
  const release = await lock(`${file}.cache-durable`, { retries: 0 });

  const box = await start(file);
  expect(box.notify).toHaveBeenCalledWith(expect.stringContaining("stays in memory"), "warning");
  await box.run("before_provider_request", request([{ role: "user", content: "a" }]));
  await box.run("message_end", answer(100, 900));
  expect(box.setStatus).toHaveBeenLastCalledWith("prompt-cache", "cache 90%");
  // One warning, not one error per event: the failed open is not retried.
  expect(box.notify).toHaveBeenCalledTimes(1);
  await box.run("session_shutdown", {});
  await release();
});
