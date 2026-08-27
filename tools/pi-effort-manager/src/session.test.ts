import { expect, it } from "vitest";
import { latestPersistedState, setSessionOverride } from "./session.ts";

it("restores the latest valid session policy and qualifying compaction count", () => {
  const entries: readonly unknown[] = [
    { type: "compaction" },
    { customType: "pi-effort-manager-state-v1", data: "invalid", type: "custom" },
    {
      customType: "pi-effort-manager-state-v1",
      data: {
        compactionCount: 2,
        enabled: true,
        overrides: {
          compactionEffort: "max",
          compactionResetEffort: "xhigh",
          compactionResetInterval: 4,
          endEffort: "xhigh",
          startEffort: "medium",
        },
        resetEffort: "xhigh",
        resetInterval: 4,
      },
      type: "custom",
    },
  ];
  expect(latestPersistedState(entries)).toStrictEqual({
    compactionCount: 2,
    enabled: true,
    overrides: {
      compactionEffort: "max",
      compactionResetEffort: "xhigh",
      compactionResetInterval: 4,
      endEffort: "xhigh",
      startEffort: "medium",
    },
    resetEffort: "xhigh",
    resetInterval: 4,
  });
  expect(latestPersistedState([])).toStrictEqual({});
  expect(
    latestPersistedState([
      {
        customType: "pi-effort-manager-state-v1",
        data: { enabled: "invalid", overrides: "invalid" },
        type: "custom",
      },
    ]),
  ).toStrictEqual({
    compactionCount: undefined,
    enabled: undefined,
    overrides: undefined,
    resetEffort: undefined,
    resetInterval: undefined,
  });
});

it("sets and clears validated session overrides", () => {
  expect(setSessionOverride({}, "startEffort", "low")).toStrictEqual({ startEffort: "low" });
  expect(setSessionOverride({ startEffort: "low" }, "startEffort", "default")).toStrictEqual({
    startEffort: undefined,
  });
  expect(setSessionOverride({ endEffort: "high" }, "endEffort", "default")).toStrictEqual({
    endEffort: undefined,
  });
  expect(
    setSessionOverride({ compactionEffort: "max" }, "compactionEffort", "default"),
  ).toStrictEqual({ compactionEffort: undefined });
  expect(
    setSessionOverride({ compactionResetEffort: "xhigh" }, "compactionResetEffort", "default"),
  ).toStrictEqual({ compactionResetEffort: undefined });
  expect(
    setSessionOverride({ compactionResetInterval: 3 }, "compactionResetInterval", "default"),
  ).toStrictEqual({ compactionResetInterval: undefined });
  expect(setSessionOverride({}, "compactionResetEffort", "high")).toStrictEqual({
    compactionResetEffort: "high",
  });
  expect(setSessionOverride({}, "compactionResetInterval", "5")).toStrictEqual({
    compactionResetInterval: 5,
  });
  expect(setSessionOverride({}, "compactionResetInterval", "zero")).toBe(
    "Compaction reset interval must be a positive integer.",
  );
  expect(setSessionOverride({}, "endEffort", "invalid")).toBe("Invalid effort level: invalid.");
});
