import type { Api, Model } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import {
  compactionLimit,
  effortAtLeast,
  effortProfile,
  selectDynamicEffort,
  shouldResetAfterCompaction,
} from "./policy.ts";

it("builds a dynamic profile with the deepest effort reserved for compaction", () => {
  const profile = effortProfile({
    id: "reasoning",
    reasoning: true,
    thinkingLevelMap: {
      off: null,
      minimal: "minimal",
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: "xhigh",
      max: "max",
    },
  } as Model<Api>);

  expect(profile).toStrictEqual({
    baseline: "medium",
    compaction: "max",
    operational: ["medium", "high", "xhigh"],
    supported: ["minimal", "low", "medium", "high", "xhigh", "max"],
  });
});

it("deduplicates provider-equivalent efforts and handles narrow models", () => {
  expect(
    effortProfile({
      id: "duplicate",
      reasoning: true,
      thinkingLevelMap: {
        off: null,
        minimal: null,
        low: "low",
        medium: "medium",
        high: "xhigh",
        xhigh: "xhigh",
        max: null,
      },
    } as Model<Api>),
  ).toStrictEqual({
    baseline: "medium",
    compaction: "xhigh",
    operational: ["medium"],
    supported: ["low", "medium", "xhigh"],
  });
  expect(
    effortProfile({
      id: "single",
      reasoning: true,
      thinkingLevelMap: {
        off: null,
        minimal: null,
        low: null,
        medium: null,
        high: null,
        xhigh: null,
        max: "max",
      },
    } as Model<Api>),
  ).toStrictEqual({
    baseline: "max",
    compaction: "max",
    operational: ["max"],
    supported: ["max"],
  });
  expect(effortProfile({ id: "plain", reasoning: false } as Model<Api>)).toBeUndefined();
  expect(effortProfile(undefined)).toBeUndefined();
  expect(
    effortProfile({
      id: "none-supported",
      reasoning: true,
      thinkingLevelMap: {
        off: null,
        minimal: null,
        low: null,
        medium: null,
        high: null,
        xhigh: null,
        max: null,
      },
    } as Model<Api>),
  ).toBeUndefined();
});

it("ramps through operational efforts as the compaction limit approaches", () => {
  const profile = {
    baseline: "low" as const,
    compaction: "max" as const,
    operational: ["low", "medium", "high", "xhigh"] as const,
    supported: ["minimal", "low", "medium", "high", "xhigh", "max"] as const,
  };

  expect(
    selectDynamicEffort({ contextTokens: 50, contextWindow: 110, profile, reserveTokens: 10 }),
  ).toBe("low");
  expect(
    selectDynamicEffort({ contextTokens: 61, contextWindow: 110, profile, reserveTokens: 10 }),
  ).toBe("medium");
  expect(
    selectDynamicEffort({ contextTokens: 75, contextWindow: 110, profile, reserveTokens: 10 }),
  ).toBe("high");
  expect(
    selectDynamicEffort({ contextTokens: 90, contextWindow: 110, profile, reserveTokens: 10 }),
  ).toBe("xhigh");
  expect(
    selectDynamicEffort({
      contextTokens: 99,
      contextWindow: 110,
      forceBaseline: true,
      profile,
      reserveTokens: 10,
    }),
  ).toBe("low");
  expect(
    selectDynamicEffort({
      contextTokens: -10,
      contextWindow: 110,
      profile,
      rampStartRatio: -1,
      reserveTokens: 10,
    }),
  ).toBe("low");
  expect(
    selectDynamicEffort({
      contextTokens: 100,
      contextWindow: 110,
      profile,
      rampStartRatio: 2,
      reserveTokens: 10,
    }),
  ).toBe("xhigh");

  const sparseOperational = ["low"] as ("low" | "medium" | "high" | "xhigh")[];
  sparseOperational.length = 4;
  expect(
    selectDynamicEffort({
      contextTokens: 100,
      contextWindow: 110,
      profile: { ...profile, operational: sparseOperational },
      reserveTokens: 10,
    }),
  ).toBe("low");
});

it("uses default mappings and defaults for dynamic selection", () => {
  const profile = effortProfile({ id: "standard", reasoning: true } as Model<Api>);
  expect(profile).toStrictEqual({
    baseline: "medium",
    compaction: "high",
    operational: ["medium"],
    supported: ["minimal", "low", "medium", "high"],
  });
  if (profile === undefined) {
    throw new Error("expected standard effort profile");
  }
  expect(
    selectDynamicEffort({
      contextTokens: 1,
      contextWindow: 20_000,
      profile,
    }),
  ).toBe("medium");
  expect(
    selectDynamicEffort({
      contextTokens: 100,
      contextWindow: 200,
      profile: {
        baseline: "low",
        compaction: "high",
        operational: ["low"],
        supported: ["low", "high"],
      },
    }),
  ).toBe("low");
});

it("clamps invalid limits and resets after every successful compaction by default", () => {
  expect(compactionLimit(100, 200)).toBe(1);
  expect(compactionLimit(20_000)).toBe(3616);
  expect(shouldResetAfterCompaction(0)).toBe(false);
  expect(shouldResetAfterCompaction(1)).toBe(true);
  expect(shouldResetAfterCompaction(2)).toBe(true);
  expect(shouldResetAfterCompaction(6)).toBe(true);
  expect(shouldResetAfterCompaction(4, 2)).toBe(true);
  expect(shouldResetAfterCompaction(4, 0)).toBe(false);
  expect(effortAtLeast("high", "xhigh")).toBe(false);
  expect(effortAtLeast("xhigh", "xhigh")).toBe(true);
  expect(effortAtLeast("max", "xhigh")).toBe(true);
  expect(effortAtLeast("off", "minimal")).toBe(false);
  expect(effortAtLeast(undefined, "minimal")).toBe(false);
});

it("applies configured start, end, and compaction effort boundaries", () => {
  const profile = effortProfile(
    {
      id: "configured",
      reasoning: true,
      thinkingLevelMap: {
        off: null,
        minimal: "minimal",
        low: "low",
        medium: "medium",
        high: "high",
        xhigh: "xhigh",
        max: "max",
      },
    } as Model<Api>,
    { compactionEffort: "xhigh", endEffort: "high", startEffort: "low" },
  );
  expect(profile).toStrictEqual({
    baseline: "low",
    compaction: "xhigh",
    operational: ["low", "medium", "high"],
    supported: ["minimal", "low", "medium", "high", "xhigh", "max"],
  });
  expect(
    effortProfile(
      {
        id: "sparse-configured",
        reasoning: true,
        thinkingLevelMap: {
          off: null,
          minimal: "low",
          low: null,
          medium: null,
          high: "high",
          xhigh: null,
          max: null,
        },
      } as Model<Api>,
      { compactionEffort: "medium", endEffort: "minimal", startEffort: "max" },
    ),
  ).toStrictEqual({
    baseline: "minimal",
    compaction: "high",
    operational: ["minimal"],
    supported: ["minimal", "high"],
  });
});
