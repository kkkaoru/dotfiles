// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import {
  CompletionDelivery,
  type CompletionDeliveryContext,
  type CompletionDeliveryHost,
} from "./delivery.ts";
import { createTmuxLaunch } from "./tmux.ts";

afterEach(() => {
  vi.useRealTimers();
});

function retryContext(idle: () => boolean): CompletionDeliveryContext {
  return { isIdle: idle, ui: { notify: vi.fn(), setStatus: vi.fn() } };
}

it("retry-flushes a deferred completion once idle without waiting for agent_settled", () => {
  vi.useFakeTimers();
  let idle = false;
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const delivery = new CompletionDelivery({ sendUserMessage });
  const launch = createTmuxLaunch({ command: "quick job", id: 1, namespace: "a".repeat(32) });
  delivery.setContext(retryContext((): boolean => idle));
  delivery.complete({ launch, completedAt: new Date().toISOString(), exitCode: 0 });

  vi.advanceTimersByTime(5000);
  expect(sendUserMessage).not.toHaveBeenCalled();
  expect(delivery.hasPending()).toBe(true);

  idle = true;
  vi.advanceTimersByTime(5000);

  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(delivery.hasPending()).toBe(false);
  vi.advanceTimersByTime(60_000);
  expect(sendUserMessage).toHaveBeenCalledOnce();
});

it("re-arms the retry flush while delivery keeps failing", () => {
  vi.useFakeTimers();
  const sendUserMessage = vi
    .fn<CompletionDeliveryHost["sendUserMessage"]>()
    .mockImplementationOnce((): void => {
      throw new Error("Agent is already processing a prompt");
    });
  const delivery = new CompletionDelivery({ sendUserMessage });
  const launch = createTmuxLaunch({ command: "slow job", id: 2, namespace: "a".repeat(32) });
  delivery.setContext(retryContext((): boolean => true));
  delivery.complete({ launch, completedAt: new Date().toISOString(), exitCode: 0 });
  expect(delivery.hasPending()).toBe(true);

  vi.advanceTimersByTime(5000);
  expect(sendUserMessage).toHaveBeenCalledTimes(2);
  expect(delivery.hasPending()).toBe(false);
});

it("stops the retry flush on clear", () => {
  vi.useFakeTimers();
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const delivery = new CompletionDelivery({ sendUserMessage });
  const launch = createTmuxLaunch({ command: "cleared job", id: 3, namespace: "a".repeat(32) });
  delivery.setContext(retryContext((): boolean => false));
  delivery.complete({ launch, completedAt: new Date().toISOString(), exitCode: 0 });
  expect(delivery.hasPending()).toBe(true);
  delivery.clear();
  delivery.clear();

  vi.advanceTimersByTime(60_000);
  expect(sendUserMessage).not.toHaveBeenCalled();
  expect(delivery.hasPending()).toBe(false);
});
