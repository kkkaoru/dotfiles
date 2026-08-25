// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import {
  CompletionDelivery,
  type CompletionDeliveryContext,
  type CompletionDeliveryHost,
  wakePiOnCompletion,
} from "./delivery.ts";

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
        statusPath: "/tmp/pi-tmux-test/exit-status",
        submittedAt: new Date(2026, 7, 26, 2, 5).toISOString(),
        taskCommand: `cat > /tmp/test.py <<'PY'\n${"print('ok') ".repeat(30)}\nPY`,
      },
    },
  );

  expect(sendUserMessage).toHaveBeenCalledWith(
    expect.stringMatching(
      /^08-26 02:05 → 02:15 \| tmux=pi-tmux-test \| cat > \/tmp\/test\.py <<'PY' print\('ok'\)[^\n]{0,160}\nlog: \/tmp\/pi-tmux-test\/output\.log\nstatus: \/tmp\/pi-tmux-test\/exit-status$/u,
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
      statusPath: "/tmp/pi-tmux-test/exit-status",
      submittedAt: new Date(2026, 7, 26, 2, 5).toISOString(),
      taskCommand: "run verification",
    },
  };
  delivery.setContext(context);

  delivery.complete(completion);

  expect(notify).toHaveBeenCalledWith(
    "08-26 02:05 → 02:15 | tmux=pi-tmux-test | failed=2 | run verification",
    "error",
  );
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", [
    "08-26 02:05 → 02:15 | tmux=pi-tmux-test | failed=2 | run verification",
  ]);

  idle = true;
  delivery.agentSettled(context);

  expect(sendUserMessage).toHaveBeenCalledOnce();
  expect(onDelivered).toHaveBeenCalledWith(completion);
  expect(setWidget).toHaveBeenLastCalledWith("tmux-completions", undefined);
});
