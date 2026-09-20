// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import {
  CompletionDelivery,
  type CompletionDeliveryContext,
  type CompletionDeliveryHost,
  wakePiOnCompletion,
} from "./delivery.ts";
import type { Completion } from "./waiter.ts";
import { createTmuxLaunch } from "./tmux.ts";

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
    { deliverAs: "followUp" },
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
    { deliverAs: "followUp" },
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
    { deliverAs: "followUp" },
  );
  expect(onDelivered).toHaveBeenCalledWith(completion);
});

it("coalesces a busy completion burst into one latest-task summary", () => {
  vi.useFakeTimers();
  let idle = false;
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const onDelivered = vi.fn();
  const context: CompletionDeliveryContext = {
    isIdle: (): boolean => idle,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const delivery = new CompletionDelivery({ sendUserMessage }, { onDelivered });
  const olderCompletion: Completion = {
    completedAt: "2026-08-26T02:12:00.000Z",
    exitCode: 1,
    launch: {
      command: "tmux command",
      completionChannel: "pi-tmux-test-older-complete",
      logPath: "/tmp/pi-tmux-test-older/output.log",
      sessionName: "pi-tmux-test-older",
      socketName: "pi-tmux-socket",
      statusPath: "/tmp/pi-tmux-test-older/exit-status",
      submittedAt: "2026-08-26T02:10:00.000Z",
      taskCommand: "older verification",
    },
  };
  const latestCompletion: Completion = {
    completedAt: "2026-08-26T02:14:00.000Z",
    exitCode: 0,
    launch: {
      command: "tmux command",
      completionChannel: "pi-tmux-test-latest-complete",
      logPath: "/tmp/pi-tmux-test-latest/output.log",
      sessionName: "pi-tmux-test-latest",
      socketName: "pi-tmux-socket",
      statusPath: "/tmp/pi-tmux-test-latest/exit-status",
      submittedAt: "2026-08-26T02:13:00.000Z",
      taskCommand: "latest verification",
    },
  };
  delivery.setContext(context);
  delivery.complete(latestCompletion);
  delivery.complete(olderCompletion);

  idle = true;
  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledWith(
    "tmux completion batch: 2 tasks finished while Pi was busy (1 succeeded, 1 failed or orphaned).\nEarlier completion details coalesced: 1. Their artifacts remain under the same Pi tmux session namespace; inspect them only if still relevant.\nlatest completion:\n11:13 → 11:14 | latest verification\nlog: /tmp/pi-tmux-test-latest/output.log\nstatus: /tmp/pi-tmux-test-latest/exit-status",
    { deliverAs: "followUp" },
  );
  expect(onDelivered).toHaveBeenCalledTimes(2);
  expect(onDelivered).toHaveBeenNthCalledWith(1, latestCompletion);
  expect(onDelivered).toHaveBeenNthCalledWith(2, olderCompletion);
});

it("defers and coalesces settled delivery until the lifecycle callback returns", () => {
  vi.useFakeTimers();
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const context: CompletionDeliveryContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
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
      submittedAt: "2026-08-26T02:14:00.000Z",
      taskCommand: "run verification",
    },
  };
  delivery.setContext({ ...context, isIdle: (): boolean => false });
  delivery.complete(completion);

  delivery.deferAgentSettled(context);
  delivery.deferAgentSettled(context);
  expect(sendUserMessage).not.toHaveBeenCalled();
  vi.runOnlyPendingTimers();

  expect(sendUserMessage).toHaveBeenCalledOnce();
});

it("cancels deferred settled delivery when cleared", () => {
  vi.useFakeTimers();
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const context: CompletionDeliveryContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const delivery = new CompletionDelivery({ sendUserMessage });
  delivery.deferAgentSettled(context);

  delivery.clear();
  vi.runOnlyPendingTimers();

  expect(sendUserMessage).not.toHaveBeenCalled();
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

it("does not redeliver when post-delivery bookkeeping fails", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const delivery = new CompletionDelivery(
    { sendUserMessage },
    {
      onDelivered: (): never => {
        throw new Error("artifact disappeared");
      },
    },
  );
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

  expect((): void => delivery.complete(completion)).not.toThrow();
  expect(sendUserMessage).toHaveBeenCalledOnce();
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

it("wakes an idle agent for an overdue job without marking completion delivered", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const onDelivered = vi.fn();
  const delivery = new CompletionDelivery({ sendUserMessage }, { onDelivered });
  const launch = createTmuxLaunch({ command: "wrangler tail", id: 1, namespace: "a".repeat(32) });
  delivery.overdue([launch]);

  expect(sendUserMessage).toHaveBeenCalledExactlyOnceWith(
    expect.stringMatching(/^tmux overdue check-in: 1 task\(s\)/u),
    { deliverAs: "followUp" },
  );
  expect(sendUserMessage.mock.calls[0]?.[0]).toMatch(
    /task: wrangler tail\ntmux socket: pi-tmux-a{32}; session: pi-tmux-a{32}-1/u,
  );
  expect(sendUserMessage.mock.calls[0]?.[0]).toMatch(
    /Use read on the exact log path below and inspect process state now/u,
  );
  expect(onDelivered).not.toHaveBeenCalled();
});

it("defers overdue check-ins across busy and compacting phases and drops completed jobs", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const onDelivered = vi.fn();
  const delivery = new CompletionDelivery({ sendUserMessage }, { onDelivered });
  const context: CompletionDeliveryContext = {
    isIdle: () => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const first = createTmuxLaunch({ command: "first watcher", id: 1, namespace: "a".repeat(32) });
  const second = createTmuxLaunch({ command: "second watcher", id: 2, namespace: "a".repeat(32) });
  delivery.setContext({ ...context, isIdle: () => false });
  delivery.overdue([first, second]);
  delivery.overdue([second]);
  expect(sendUserMessage).not.toHaveBeenCalled();
  delivery.beforeCompaction();
  delivery.complete({ launch: first, completedAt: new Date().toISOString(), exitCode: 0 });
  delivery.agentSettled(context);
  expect(sendUserMessage).not.toHaveBeenCalled();
  delivery.afterCompaction(context);

  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(sendUserMessage.mock.calls[0]?.[0]).toMatch(/tmux overdue check-in: 1 task\(s\)/u);
  expect(sendUserMessage.mock.calls[0]?.[0]).toMatch(/task: second watcher/u);
  expect(sendUserMessage.mock.calls[0]?.[0]).not.toMatch(/task: first watcher/u);
  expect(onDelivered).toHaveBeenCalledOnce();
});

it("delivers a pending notice once the agent settles even while it stays busy", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const context: CompletionDeliveryContext = {
    isIdle: (): boolean => false,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const delivery = new CompletionDelivery({ sendUserMessage });
  const launch = createTmuxLaunch({ command: "long watcher", id: 1, namespace: "a".repeat(32) });
  delivery.setContext(context);
  delivery.overdue([launch]);
  expect(sendUserMessage).not.toHaveBeenCalled();
  expect(delivery.pendingTaskNames()).toStrictEqual([launch.sessionName]);

  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(sendUserMessage.mock.calls[0]?.[0]).toMatch(/tmux overdue check-in/u);
  expect(delivery.pendingTaskNames()).toStrictEqual([]);
});

it("lists task names with queued completion and overdue notices", () => {
  const delivery = new CompletionDelivery({ sendUserMessage: vi.fn() });
  const first = createTmuxLaunch({ command: "first", id: 1, namespace: "a".repeat(32) });
  const second = createTmuxLaunch({ command: "second", id: 2, namespace: "a".repeat(32) });
  delivery.setContext({
    isIdle: (): boolean => false,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  });
  delivery.complete({ launch: first, completedAt: new Date().toISOString(), exitCode: 0 });
  delivery.overdue([second]);

  expect(delivery.pendingTaskNames()).toStrictEqual([first.sessionName, second.sessionName]);
});

it("retains an overdue check-in on delivery races and clears it at shutdown", () => {
  const sendUserMessage = vi
    .fn<CompletionDeliveryHost["sendUserMessage"]>()
    .mockImplementationOnce(() => {
      throw new Error("Agent is already processing a prompt");
    });
  const delivery = new CompletionDelivery({ sendUserMessage });
  const context: CompletionDeliveryContext = {
    isIdle: () => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const launch = createTmuxLaunch({ command: "watcher", id: 1, namespace: "a".repeat(32) });
  delivery.overdue([launch]);
  delivery.agentSettled(context);
  expect(sendUserMessage).toHaveBeenCalledTimes(2);
  delivery.beforeCompaction();
  delivery.overdue([launch]);
  delivery.clear();
  delivery.afterCompaction(context);
  expect(sendUserMessage).toHaveBeenCalledTimes(2);
});

it("bounds overdue batches without losing remaining task notices", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const delivery = new CompletionDelivery({ sendUserMessage });
  const context: CompletionDeliveryContext = {
    isIdle: () => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const launches = Array.from({ length: 21 }, (_value, id) =>
    createTmuxLaunch({ command: "watcher", id, namespace: "a".repeat(32) }),
  );
  delivery.overdue(launches);
  expect(sendUserMessage).toHaveBeenCalledExactlyOnceWith(
    expect.stringMatching(/^tmux overdue check-in: 20 task\(s\)/u),
    { deliverAs: "followUp" },
  );
  delivery.agentSettled(context);
  expect(sendUserMessage).toHaveBeenLastCalledWith(
    expect.stringMatching(/^tmux overdue check-in: 1 task\(s\)/u),
    { deliverAs: "followUp" },
  );
  delivery.agentSettled(context);
  expect(sendUserMessage).toHaveBeenCalledTimes(2);
});
