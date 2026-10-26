// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import type { TmuxExtensionHost, TmuxToolDefinition } from "../index.ts";
import { ACTIVE_DISPLAY_ENTRY_TYPE } from "./active-display.ts";
import { type CheckInDependencies, recordCheckIn, registerCheckInTool } from "./check-in.ts";
import { createTmuxLaunch, type TmuxLaunch } from "./tmux.ts";

const LAUNCH: TmuxLaunch = createTmuxLaunch({
  command: "python3 edits/work/watch_ex34.py",
  id: 1,
  namespace: "a".repeat(32),
});

function fixture(): {
  readonly clearAcknowledged: ReturnType<typeof vi.fn>;
  readonly dependencies: CheckInDependencies;
  readonly persist: ReturnType<typeof vi.fn>;
  readonly reschedule: ReturnType<typeof vi.fn>;
  readonly resolved: ReturnType<typeof vi.fn>;
  readonly state: ReturnType<typeof vi.fn>;
} {
  const resolved = vi.fn();
  const clearAcknowledged = vi.fn(() => true);
  const state = vi.fn(() => ({ dismissedSessionNames: [], hidden: false }));
  const reschedule = vi.fn<
    (sessionName: string, nextCheckSeconds: number) => TmuxLaunch | undefined
  >(() => ({ ...LAUNCH, estimatedCompletionAt: new Date(Date.now() + 1_800_000).toISOString() }));
  const persist = vi.fn();
  return {
    clearAcknowledged,
    dependencies: {
      delivery: { resolved },
      display: { clearAcknowledged, state },
      persist,
      runtime: { reschedule },
    },
    persist,
    reschedule,
    resolved,
    state,
  };
}

afterEach(() => {
  vi.useRealTimers();
});

it("records a bounded next check and clears the overdue notice", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 9, 26, 23, 0));
  const test = fixture();

  const result = recordCheckIn(test.dependencies, LAUNCH.sessionName, 1800);

  expect(test.reschedule).toHaveBeenCalledWith(LAUNCH.sessionName, 1800, expect.any(Number));
  expect(test.resolved).toHaveBeenCalledExactlyOnceWith([result.launch]);
  expect(test.clearAcknowledged).toHaveBeenCalledExactlyOnceWith([result.launch]);
  expect(test.persist).toHaveBeenCalledExactlyOnceWith(ACTIVE_DISPLAY_ENTRY_TYPE, {
    dismissedSessionNames: [],
    hidden: false,
  });
  expect(result.text).toContain("tmux check-in recorded");
  expect(result.text).toContain("Next check-in: 10-26 23:30 (30 min).");
  expect(result.text).toContain("does not stop it or prove completion");
});

it("does not persist display state when the job had no checked marker", () => {
  const test = fixture();
  test.clearAcknowledged.mockReturnValue(false);

  recordCheckIn(test.dependencies, LAUNCH.sessionName, 600);

  expect(test.persist).not.toHaveBeenCalled();
});

it("rejects a check-in for a job that is no longer tracked", () => {
  const test = fixture();
  test.reschedule.mockReturnValue(undefined);

  expect(() => recordCheckIn(test.dependencies, LAUNCH.sessionName, 600)).toThrow(
    /No active tmux task named/u,
  );
  expect(test.resolved).not.toHaveBeenCalled();
});

it("registers a check-in tool that refreshes the launch and flushes persistence", async () => {
  let tool: TmuxToolDefinition | undefined;
  const flush = vi.fn(async () => undefined);
  const host: TmuxExtensionHost = {
    exec: vi.fn(),
    flush,
    on: vi.fn(),
    registerTool: (definition): void => {
      tool = definition;
    },
    sendUserMessage: vi.fn(),
  };
  const test = fixture();
  registerCheckInTool(host, test.dependencies);

  expect(tool?.name).toBe("tmux_check_in");
  expect(tool?.parameters).toMatchObject({ required: ["sessionName", "nextCheckSeconds"] });
  expect(tool?.promptGuidelines.join(" ")).toContain("tmux_check_in");
  const result = await tool?.execute(
    "call-1",
    { nextCheckSeconds: 1800, sessionName: LAUNCH.sessionName },
    undefined,
  );
  expect(test.reschedule).toHaveBeenCalledWith(LAUNCH.sessionName, 1800, expect.any(Number));
  expect(result?.details.sessionName).toBe(LAUNCH.sessionName);
  expect(result?.content[0].text).toMatch(/^tmux check-in recorded/u);
  expect(flush).toHaveBeenCalledOnce();
});

it("propagates a failed check-in through the tool", async () => {
  let tool: TmuxToolDefinition | undefined;
  const host: TmuxExtensionHost = {
    exec: vi.fn(),
    on: vi.fn(),
    registerTool: (definition): void => {
      tool = definition;
    },
    sendUserMessage: vi.fn(),
  };
  const test = fixture();
  test.reschedule.mockReturnValue(undefined);
  registerCheckInTool(host, test.dependencies);

  await expect(
    tool?.execute("call-1", { nextCheckSeconds: 600, sessionName: LAUNCH.sessionName }, undefined),
  ).rejects.toThrow(/No active tmux task named/u);
});
