// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import {
  CompletionDelivery,
  type CompletionDeliveryContext,
  type CompletionDeliveryHost,
  wakePiOnCompletion,
} from "./delivery.ts";
import type { Completion } from "./waiter.ts";

afterEach(() => {
  vi.useRealTimers();
});

it("normalizes completion identity into one bounded naming line", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 7, 26, 2, 15));
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();

  wakePiOnCompletion(
    { sendUserMessage },
    {
      exitCode: 0,
      launch: {
        command: "tmux command",
        completionChannel: "pi-tmux-test-complete",
        logPath: "/tmp/pi-tmux-test/output.log",
        sessionName: "pi-tmux-test",
        socketName: "pi-tmux-socket",
        statusPath: "/tmp/pi-tmux-test/exit-status",
        submittedAt: new Date(2026, 7, 25, 23, 55).toISOString(),
        taskCommand: `cat > /tmp/test.py <<'PY'\n${"print('ok') ".repeat(30)}\nPY`,
      },
    },
  );

  expect(sendUserMessage).toHaveBeenCalledWith(
    expect.stringMatching(
      /^08-25 23:55 → 08-26 02:15 \| cat > \/tmp\/test\.py <<'PY' print\('ok'\)[^\n]{0,160}\nlog: \/tmp\/pi-tmux-test\/output\.log\nstatus: \/tmp\/pi-tmux-test\/exit-status$/u,
    ),
  );
});

it("shows a named completion while busy and wakes without a Follow-up queue after settling", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 7, 26, 2, 15));
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const notify = vi.fn<CompletionDeliveryContext["ui"]["notify"]>();
  const setStatus = vi.fn<CompletionDeliveryContext["ui"]["setStatus"]>();
  const setWidget = vi.fn<NonNullable<CompletionDeliveryContext["ui"]["setWidget"]>>();
  let idle = false;
  const context: CompletionDeliveryContext = {
    isIdle: (): boolean => idle,
    ui: { notify, setStatus, setWidget },
  };
  const onDelivered = vi.fn();
  const delivery = new CompletionDelivery({ sendUserMessage }, { onDelivered });
  const completion = {
    exitCode: 2,
    launch: {
      command: "tmux command",
      completionChannel: "pi-tmux-test-complete",
      logPath: "/tmp/pi-tmux-test/output.log",
      sessionName: "pi-tmux-test",
      socketName: "pi-tmux-socket",
      statusPath: "/tmp/pi-tmux-test/exit-status",
      submittedAt: new Date(2026, 7, 26, 2, 5).toISOString(),
      taskCommand: "run verification",
    },
  };
  delivery.setContext(context);

  delivery.complete(completion);

  expect(notify).toHaveBeenCalledWith(
    "02:05 → 02:15 | command_exit=2 | run verification",
    "warning",
  );
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", [
    "02:05 → 02:15 | command_exit=2 | run verification",
  ]);

  idle = true;
  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(onDelivered).toHaveBeenCalledWith(completion);
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", undefined);
});

it("requeues a completion when sendUserMessage races with an active agent", () => {
  const sendUserMessage = vi
    .fn<CompletionDeliveryHost["sendUserMessage"]>()
    .mockImplementationOnce((): never => {
      throw new Error("Agent is already processing a prompt. Use steer() or followUp().");
    });
  const notify = vi.fn<CompletionDeliveryContext["ui"]["notify"]>();
  const setStatus = vi.fn<CompletionDeliveryContext["ui"]["setStatus"]>();
  const setWidget = vi.fn<NonNullable<CompletionDeliveryContext["ui"]["setWidget"]>>();
  const context: CompletionDeliveryContext = {
    isIdle: (): boolean => true,
    ui: { notify, setStatus, setWidget },
  };
  const onDelivered = vi.fn();
  const delivery = new CompletionDelivery({ sendUserMessage }, { onDelivered });
  const completion: Completion = {
    exitCode: 0,
    launch: {
      command: "tmux command",
      completionChannel: "pi-tmux-test-complete",
      logPath: "/tmp/pi-tmux-test/output.log",
      sessionName: "pi-tmux-test",
      socketName: "pi-tmux-socket",
      statusPath: "/tmp/pi-tmux-test/exit-status",
      submittedAt: new Date().toISOString(),
      taskCommand: "run verification",
    },
  };
  delivery.setContext(context);

  expect((): void => delivery.complete(completion)).not.toThrow();
  expect(onDelivered).not.toHaveBeenCalled();
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", [
    expect.stringContaining("run verification"),
  ]);

  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledTimes(2);
  expect(onDelivered).toHaveBeenCalledWith(completion);
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", undefined);
});

it("does not hide unrelated completion delivery errors", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>(() => {
    throw new Error("unexpected delivery failure");
  });
  const delivery = new CompletionDelivery({ sendUserMessage });
  const completion: Completion = {
    exitCode: 0,
    launch: {
      command: "tmux command",
      completionChannel: "pi-tmux-test-complete",
      logPath: "/tmp/pi-tmux-test/output.log",
      sessionName: "pi-tmux-test",
      socketName: "pi-tmux-socket",
      statusPath: "/tmp/pi-tmux-test/exit-status",
      submittedAt: new Date().toISOString(),
      taskCommand: "run verification",
    },
  };

  expect((): void => delivery.complete(completion)).toThrow("unexpected delivery failure");
});
