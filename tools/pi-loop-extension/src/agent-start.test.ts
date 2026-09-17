// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import { registerAgentLoop, type StartLoopToolDefinition } from "./agent-start.ts";
import { LoopRuntime, type LoopContext, type Scheduler } from "./runtime.ts";

const context: LoopContext = { isIdle: () => false, ui: { notify: vi.fn(), setStatus: vi.fn() } };
const scheduler: Scheduler = {
  now: () => Date.now(),
  setInterval: (callback, milliseconds) => globalThis.setInterval(callback, milliseconds),
  clearInterval: (timer) => globalThis.clearInterval(timer),
};
afterEach(() => vi.useRealTimers());

it("adopts the current turn without queuing a duplicate initial prompt", async () => {
  const sendUserMessage = vi.fn();
  const appendEntry = vi.fn();
  const runtime = new LoopRuntime({ sendUserMessage, appendEntry });
  const tools: StartLoopToolDefinition[] = [];
  registerAgentLoop(
    {
      registerTool: (tool) => {
        tools.push(tool);
      },
    },
    runtime,
  );
  const result = await tools[0]?.execute(
    "call",
    { prompt: "  Verify the user's build  " },
    undefined,
    undefined,
    context,
  );
  expect(result).toMatchObject({ details: { prompt: "Verify the user's build" } });
  expect(result?.content[0].text).toMatch(/exactly one terminal loop decision/u);
  expect(sendUserMessage).not.toHaveBeenCalled();
  expect(runtime.ownsContinuation()).toBe(true);
  expect(appendEntry.mock.lastCall?.[1]).toMatchObject({ paused: false, pendingContinuations: [] });
  runtime.complete("verified", context);
  runtime.agentSettled({ ...context, isIdle: () => true });
  expect(sendUserMessage).not.toHaveBeenCalled();
  runtime.shutdown();
});

it("refuses empty, duplicate and paused agent loops", () => {
  const runtime = new LoopRuntime({ sendUserMessage: vi.fn() });
  expect(() => runtime.startFromAgent(" ", context)).toThrow("non-empty");
  runtime.startFromAgent("check", context);
  expect(() => runtime.startFromAgent("replacement", context)).toThrow("existing or paused");
  runtime.command("pause", context);
  expect(() => runtime.startFromAgent("bypass", context)).toThrow("existing or paused");
  runtime.shutdown();
});

it("uses the ordinary self-paced continuation when the agent omits a terminal decision", () => {
  const sendUserMessage = vi.fn();
  const runtime = new LoopRuntime({ sendUserMessage });
  runtime.startFromAgent("finish authorized work", context);
  runtime.agentSettled({ ...context, isIdle: () => true });
  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(sendUserMessage.mock.lastCall?.[0]).toMatch(/Task:\nfinish authorized work/u);
  runtime.shutdown();
});

it("schedules one later tick after autonomous start without duplicating the current turn", () => {
  vi.useFakeTimers();
  const sendUserMessage = vi.fn();
  const runtime = new LoopRuntime({ sendUserMessage }, scheduler);
  runtime.startFromAgent("check later", context);
  runtime.wakeup({ delaySeconds: 60, prompt: "inspect result", reason: "existing job" }, context);
  runtime.agentSettled({ ...context, isIdle: () => true });
  expect(sendUserMessage).not.toHaveBeenCalled();
  vi.advanceTimersByTime(60_000);
  expect(sendUserMessage).toHaveBeenCalledOnce();
  runtime.shutdown();
});
