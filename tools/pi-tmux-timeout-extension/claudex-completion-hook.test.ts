// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import {
  type BackgroundTaskCompletion,
  backgroundTaskFromHook,
  completionMessage,
  type CompletionOperations,
  type CompletionWatcher,
  waitForBackgroundTask,
} from "./claudex-completion-hook.ts";

it("extracts a Claude Code background Bash task from nested tool output", () => {
  expect(
    backgroundTaskFromHook({
      hook_event_name: "PostToolUse",
      tool_input: { command: "bun run check", timeout: 120_000 },
      tool_name: "Bash",
      tool_response: {
        content: [
          {
            text: "Command running in background with ID: task123. Output is being written to: /tmp/tasks/task123.output. You will be notified when it completes.",
          },
        ],
      },
    }),
  ).toStrictEqual({
    command: "bun run check",
    outputPath: "/tmp/tasks/task123.output",
    taskId: "task123",
  });
});

it("ignores unrelated and malformed hook payloads", () => {
  expect(backgroundTaskFromHook(null)).toBeUndefined();
  expect(
    backgroundTaskFromHook({ hook_event_name: "PreToolUse", tool_name: "Bash" }),
  ).toBeUndefined();
  expect(
    backgroundTaskFromHook({
      hook_event_name: "PostToolUse",
      tool_input: {},
      tool_name: "Bash",
      tool_response: ["foreground result"],
    }),
  ).toBeUndefined();
});

it("wakes from a filesystem event with the completed exit status", async () => {
  const close = vi.fn<CompletionWatcher["close"]>();
  const onError = vi.fn<CompletionWatcher["onError"]>();
  const output = vi
    .fn<CompletionOperations["readFile"]>()
    .mockResolvedValueOnce("still running\n")
    .mockResolvedValueOnce("CLAUDEX_LIFECYCLE_OK\n[exited with code 0]\n");
  const changeListeners: (() => void)[] = [];
  const watch = vi.fn<CompletionOperations["watch"]>((_filePath, onChange): CompletionWatcher => {
    changeListeners.push(onChange);
    return { close, onError };
  });
  const completion = waitForBackgroundTask(
    { command: "bun run check", outputPath: "/tmp/task.output", taskId: "task123" },
    { readFile: output, watch },
  );

  await Promise.resolve();
  changeListeners[0]?.();

  await expect(completion).resolves.toStrictEqual({
    command: "bun run check",
    exitCode: 0,
    outputPath: "/tmp/task.output",
    taskId: "task123",
  });
  expect(watch).toHaveBeenCalledWith("/tmp/task.output", expect.any(Function));
  expect(onError).toHaveBeenCalledOnce();
  expect(close).toHaveBeenCalledOnce();
});

it("rejects when the completion watcher fails", async () => {
  const watch = vi.fn<CompletionOperations["watch"]>((_filePath, _onChange): CompletionWatcher => ({
    close: vi.fn(),
    onError: (listener): void => listener(new Error("watch failed")),
  }));

  await expect(
    waitForBackgroundTask(
      { command: "Bash", outputPath: "/tmp/task.output", taskId: "task123" },
      { readFile: vi.fn().mockResolvedValue("running"), watch },
    ),
  ).rejects.toThrow("watch failed");
});

it("formats a local-time completion request for the same SubAgent", () => {
  const completion: BackgroundTaskCompletion = {
    command: "bun run check",
    exitCode: 7,
    outputPath: "/tmp/task.output",
    taskId: "task123",
  };

  expect(completionMessage(completion, new Date(2026, 7, 26, 0, 45))).toBe(
    "08-26 00:45 | task=task123 | failed=7 | bun run check\n/tmp/task.output\nInspect this exact output in the same SubAgent context. Do not TaskStop, terminate, or launch another Agent.",
  );
});

it("omits successful exit status from the completion name", () => {
  expect(
    completionMessage(
      { command: "bun test", exitCode: 0, outputPath: "/tmp/task.output", taskId: "task-ok" },
      new Date(2026, 7, 26, 0, 45),
    ),
  ).toMatch(/^08-26 00:45 \| task=task-ok \| bun test\n/u);
});
