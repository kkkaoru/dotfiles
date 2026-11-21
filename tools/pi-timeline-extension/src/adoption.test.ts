// This TypeScript file is executed with Bun.
import { expect, test, vi } from "vitest";
import { start, type Harness } from "./harness.ts";

/** One tool call after another, as pi delivers them, since the count of steps is the point. */
async function stepCalls(harness: Harness, times: number): Promise<void> {
  if (times === 0) {
    return;
  }
  await harness.handlers.get("tool_call")?.({ toolName: "read" }, harness.context);
  await stepCalls(harness, times - 1);
}

test("a finished tree leaves nothing on screen", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  await harness.run({ action: "update", id: "t1", status: "done" });
  expect(harness.setWidget).toHaveBeenLastCalledWith("timeline", undefined, {
    placement: "belowEditor",
  });
});

test("the age tick lives only while a tree is on screen", async () => {
  vi.useFakeTimers({ toFake: ["Date", "clearInterval", "setInterval"] });
  try {
    const harness = await start();
    // A session that has not planned anything has no age to move.
    expect(vi.getTimerCount()).toBe(0);
    await harness.run({ action: "add", title: "Fix bug" });
    expect(vi.getTimerCount()).toBe(1);
    await harness.run({ action: "update", id: "t1", status: "done" });
    expect(vi.getTimerCount()).toBe(0);
    // The next tree brings the tick back with it.
    await harness.run({ action: "add", title: "Next" });
    expect(vi.getTimerCount()).toBe(1);
  } finally {
    vi.useRealTimers();
  }
});

test("a session that never records a tree is asked once", async () => {
  const harness = await start();
  await stepCalls(harness, 7);
  expect(harness.sendMessage).not.toHaveBeenCalled();
  await stepCalls(harness, 1);
  expect(harness.sendMessage).toHaveBeenCalledWith(
    {
      content: expect.stringContaining("no task tree yet") as string,
      customType: "timeline-nudge",
      display: false,
    },
    { deliverAs: "steer" },
  );
  await stepCalls(harness, 40);
  expect(harness.sendMessage).toHaveBeenCalledTimes(1);
});

test("a session that already planned its work is not asked to start a tree", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  await stepCalls(harness, 8);
  expect(harness.sendMessage).not.toHaveBeenCalled();
});
