// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { loopStatus, loopWidgetLines } from "./display.ts";

it("renders loop status variants", () => {
  expect(loopStatus(0, 0, false)).toBeUndefined();
  expect(loopStatus(1, 0, false)).toBe("loop: 1");
  expect(loopStatus(1, 2, true)).toBe("loop: 1 (paused), ready: 2");
});

it("renders scheduled and ready loop names without a queue prefix", () => {
  const submittedAt: number = new Date(2026, 7, 26, 2, 50).getTime();
  expect(
    loopWidgetLines(
      [
        {
          id: 1,
          nextRunAt: new Date(2026, 7, 26, 2, 52).getTime(),
          reason: "Wait for training output",
          submittedAt,
        },
        {
          id: 2,
          nextRunAt: 0,
          reason: "Paused check",
          remainingMs: 180_000,
          submittedAt,
        },
      ],
      ["08-26 02:49 → 02:50 | loop=self-paced | verify model\nfull prompt"],
      new Date(2026, 7, 26, 2, 51).getTime(),
    ),
  ).toEqual([
    "08-26 02:50 → 02:52 | loop=#1 | Wait for training output",
    "08-26 02:50 → 02:54 | loop=#2 | Paused check",
    "08-26 02:49 → 02:50 | loop=self-paced | verify model",
  ]);
});
