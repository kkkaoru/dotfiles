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
      completedAt: new Date(2026, 7, 26, 2, 15).toISOString(),
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

it("labels orphaned task completion without an invented exit result", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();

  wakePiOnCompletion(
    { sendUserMessage },
    {
      completedAt: "2026-08-26T02:15:00.000Z",
      exitCode: 255,
      launch: {
        command: "tmux command",
        completionChannel: "pi-tmux-test-complete",
        logPath: "/tmp/pi-tmux-test/output.log",
        sessionName: "pi-tmux-test",
        socketName: "pi-tmux-socket",
        statusPath: "/tmp/pi-tmux-test/exit-status",
        submittedAt: "2026-08-26T02:14:00.000Z",
        taskCommand: "run verification",
      },
      orphaned: true,
    },
  );

  expect(sendUserMessage).toHaveBeenCalledWith(
    "11:14 → 11:15 | orphaned | run verification\nlog: /tmp/pi-tmux-test/output.log\nstatus: /tmp/pi-tmux-test/exit-status",
  );
});

it("shows a transient completion while busy and delivers normally after settling", () => {
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
    completedAt: new Date(2026, 7, 26, 2, 12).toISOString(),
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
    "02:05 → 02:12 | command_exit=2 | run verification",
    "warning",
  );
  expect(sendUserMessage).not.toHaveBeenCalled();
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", undefined);

  idle = true;
  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledWith(
    "02:05 → 02:12 | command_exit=2 | run verification\nlog: /tmp/pi-tmux-test/output.log\nstatus: /tmp/pi-tmux-test/exit-status",
  );
  expect(onDelivered).toHaveBeenCalledWith(completion);
});

it("defers an immediate delivery race without showing a persistent widget", () => {
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
    completedAt: "2026-08-26T02:15:00.000Z",
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
  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(onDelivered).not.toHaveBeenCalled();
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", undefined);

  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledTimes(2);
  expect(onDelivered).toHaveBeenCalledWith(completion);
});

it("retains a completion across repeated immediate delivery races", () => {
  const sendUserMessage = vi
    .fn<CompletionDeliveryHost["sendUserMessage"]>()
    .mockImplementationOnce((): never => {
      throw new Error("Agent is already processing a prompt");
    })
    .mockImplementationOnce((): never => {
      throw new Error("Agent is already processing a prompt");
    });
  const notify = vi.fn<CompletionDeliveryContext["ui"]["notify"]>();
  const setStatus = vi.fn<CompletionDeliveryContext["ui"]["setStatus"]>();
  const setWidget = vi.fn<NonNullable<CompletionDeliveryContext["ui"]["setWidget"]>>();
  const context: CompletionDeliveryContext = {
    isIdle: (): boolean => true,
    ui: { notify, setStatus, setWidget },
  };
  const onDelivered = vi.fn();
  const completion: Completion = {
    completedAt: "2026-08-26T02:15:00.000Z",
    exitCode: 0,
    launch: {
      command: "tmux command",
      completionChannel: "pi-tmux-test-complete",
      logPath: "/tmp/pi-tmux-test/output.log",
      sessionName: "pi-tmux-test",
      socketName: "pi-tmux-socket",
      statusPath: "/tmp/pi-tmux-test/exit-status",
      submittedAt: "2026-08-26T02:14:00.000Z",
      taskCommand: "run verification",
    },
  };
  const delivery = new CompletionDelivery({ sendUserMessage }, { onDelivered });
  delivery.setContext(context);

  delivery.complete(completion);

  expect(onDelivered).not.toHaveBeenCalled();
  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", undefined);

  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledTimes(2);
  expect(onDelivered).not.toHaveBeenCalled();
  expect(notify).toHaveBeenCalledOnce();
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", undefined);

  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledTimes(3);
});

it("does not hide unrelated completion delivery errors", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>(() => {
    throw new Error("unexpected delivery failure");
  });
  const delivery = new CompletionDelivery({ sendUserMessage });
  const completion: Completion = {
    completedAt: "2026-08-26T02:15:00.000Z",
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
