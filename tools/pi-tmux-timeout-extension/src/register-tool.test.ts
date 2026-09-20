// This TypeScript file is executed with Bun. All shell and filesystem operations are mocked.
import { afterEach, expect, it, vi } from "vitest";
import type { TmuxExtensionHost, TmuxToolDefinition } from "../index.ts";
import { registerTmuxTool } from "./register-tool.ts";
import { TmuxRuntime } from "./tmux.ts";

afterEach(() => {
  vi.useRealTimers();
});
it("registers the unchanged tool independently of lifecycle wiring", async () => {
  vi.useFakeTimers();
  const tools: TmuxToolDefinition[] = [];
  const host: TmuxExtensionHost = {
    on: vi.fn(),
    sendUserMessage: vi.fn(),
    registerTool: (tool) => {
      tools.push(tool);
    },
    exec: vi.fn<TmuxExtensionHost["exec"]>().mockResolvedValue({ code: 0, stdout: "", stderr: "" }),
  };
  const runtime: TmuxRuntime = new TmuxRuntime({
    onComplete: vi.fn(),
    events: { subscribe: (): (() => void) => (): void => undefined },
    operations: { read: () => "", isRunning: () => true },
  });
  registerTmuxTool(host, runtime);
  const result = await tools[0]?.execute("call", { command: "printf ok" }, undefined);
  expect(result?.details.taskCommand).toBe("printf ok");
  expect(host.exec).toHaveBeenCalledTimes(1);
  runtime.clear();
});
it("fails the launch when pi killed it on timeout despite exit code 0", async () => {
  vi.useFakeTimers();
  const tools: TmuxToolDefinition[] = [];
  const host: TmuxExtensionHost = {
    on: vi.fn(),
    sendUserMessage: vi.fn(),
    registerTool: (tool) => {
      tools.push(tool);
    },
    exec: vi
      .fn<TmuxExtensionHost["exec"]>()
      .mockResolvedValue({ code: 0, killed: true, stdout: "", stderr: "" }),
  };
  const runtime: TmuxRuntime = new TmuxRuntime({
    onComplete: vi.fn(),
    events: { subscribe: (): (() => void) => (): void => undefined },
    operations: { read: () => "", isRunning: () => true },
  });
  registerTmuxTool(host, runtime);
  await expect(tools[0]?.execute("call", { command: "printf ok" }, undefined)).rejects.toThrow(
    /timed out/u,
  );
  runtime.clear();
});
