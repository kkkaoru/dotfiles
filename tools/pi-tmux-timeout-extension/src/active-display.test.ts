// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import {
  ACTIVE_DISPLAY_ENTRY_TYPE,
  ActiveTaskDisplay,
  recoverActiveTaskDisplayState,
} from "./active-display.ts";
import type { CompletionDeliveryContext } from "./delivery.ts";

it("shows active tmux tasks until they complete", () => {
  const notify = vi.fn<CompletionDeliveryContext["ui"]["notify"]>();
  const setStatus = vi.fn<CompletionDeliveryContext["ui"]["setStatus"]>();
  const setWidget = vi.fn<NonNullable<CompletionDeliveryContext["ui"]["setWidget"]>>();
  const context: CompletionDeliveryContext = {
    isIdle: (): boolean => true,
    ui: { notify, setStatus, setWidget },
  };
  const display = new ActiveTaskDisplay();
  const launch = {
    command: "tmux command",
    completionChannel: "pi-tmux-test-complete",
    estimatedCompletionAt: "2026-08-26T02:20:00.000Z",
    logPath: "/tmp/pi-tmux-test/output.log",
    sessionName: "pi-tmux-test",
    socketName: "pi-tmux-socket",
    statusPath: "/tmp/pi-tmux-test/exit-status",
    submittedAt: "2026-08-26T02:14:00.000Z",
    taskCommand: "run   a long\nverification",
  };

  display.update([launch]);
  expect(setStatus).not.toHaveBeenCalled();
  display.setContext(context);

  expect(setStatus).toHaveBeenLastCalledWith("tmux-running", "tmux:1");
  expect(setWidget).toHaveBeenLastCalledWith("tmux-running-tasks", [
    "⏳ 08-26 11:14 → 08-26 11:20 run a long verification",
  ]);

  display.update([
    {
      command: "tmux command",
      completionChannel: "pi-tmux-legacy-complete",
      logPath: "/tmp/pi-tmux-legacy/output.log",
      sessionName: "pi-tmux-legacy",
      socketName: "pi-tmux-socket",
      statusPath: "/tmp/pi-tmux-legacy/exit-status",
      submittedAt: "2026-08-26T02:14:00.000Z",
      taskCommand: "legacy job",
    },
  ]);
  expect(setWidget).toHaveBeenLastCalledWith("tmux-running-tasks", [
    "⏳ 08-26 11:14 → 08-26 11:16 legacy job",
  ]);

  display.update([]);
  expect(setStatus).toHaveBeenLastCalledWith("tmux-running", undefined);
  expect(setWidget).toHaveBeenLastCalledWith("tmux-running-tasks", undefined);

  display.update([launch]);
  display.clear();
  expect(setStatus).toHaveBeenLastCalledWith("tmux-running", undefined);
  expect(setWidget).toHaveBeenLastCalledWith("tmux-running-tasks", undefined);
});

it("recovers the latest valid session display state", () => {
  const entries: readonly unknown[] = [
    null,
    {},
    { type: "message" },
    { type: "custom" },
    { customType: "other", type: "custom" },
    { customType: ACTIVE_DISPLAY_ENTRY_TYPE, type: "custom" },
    { customType: ACTIVE_DISPLAY_ENTRY_TYPE, data: null, type: "custom" },
    { customType: ACTIVE_DISPLAY_ENTRY_TYPE, data: {}, type: "custom" },
    {
      customType: ACTIVE_DISPLAY_ENTRY_TYPE,
      data: { dismissedSessionNames: [], hidden: "yes" },
      type: "custom",
    },
    {
      customType: ACTIVE_DISPLAY_ENTRY_TYPE,
      data: { hidden: false },
      type: "custom",
    },
    {
      customType: ACTIVE_DISPLAY_ENTRY_TYPE,
      data: { dismissedSessionNames: "task", hidden: false },
      type: "custom",
    },
    {
      customType: ACTIVE_DISPLAY_ENTRY_TYPE,
      data: { dismissedSessionNames: [42], hidden: false },
      type: "custom",
    },
    {
      customType: ACTIVE_DISPLAY_ENTRY_TYPE,
      data: { dismissedSessionNames: ["old-task"], hidden: true },
      type: "custom",
    },
  ];

  expect(recoverActiveTaskDisplayState([])).toStrictEqual({
    dismissedSessionNames: [],
    hidden: false,
  });
  expect(recoverActiveTaskDisplayState(entries)).toStrictEqual({
    dismissedSessionNames: ["old-task"],
    hidden: true,
  });
});

it("controls and persists visible tasks independently from tracking", () => {
  const notify = vi.fn<CompletionDeliveryContext["ui"]["notify"]>();
  const setStatus = vi.fn<CompletionDeliveryContext["ui"]["setStatus"]>();
  const setWidget = vi.fn<NonNullable<CompletionDeliveryContext["ui"]["setWidget"]>>();
  const display = new ActiveTaskDisplay();
  display.setContext({
    isIdle: (): boolean => true,
    ui: { notify, setStatus, setWidget },
  });
  display.update([
    {
      command: "tmux command",
      completionChannel: "pi-tmux-old-complete",
      logPath: "/tmp/pi-tmux-old/output.log",
      sessionName: "pi-tmux-old",
      socketName: "pi-tmux-socket",
      statusPath: "/tmp/pi-tmux-old/exit-status",
      submittedAt: "2026-08-26T02:14:00.000Z",
      taskCommand: "old job",
    },
  ]);

  expect(display.activeCount()).toBe(1);
  expect(display.visibleCount()).toBe(1);
  expect(display.dismissActive()).toBe(1);
  expect(display.visibleCount()).toBe(0);
  expect(display.state()).toStrictEqual({
    dismissedSessionNames: ["pi-tmux-old"],
    hidden: false,
  });

  display.reset();
  expect(display.visibleCount()).toBe(1);
  display.setHidden(true);
  expect(display.visibleCount()).toBe(0);
  display.setHidden(false);
  expect(display.visibleCount()).toBe(1);
  display.restore({ dismissedSessionNames: ["pi-tmux-old"], hidden: false });
  expect(display.visibleCount()).toBe(0);
  expect(setWidget).toHaveBeenLastCalledWith("tmux-running-tasks", undefined);
});
