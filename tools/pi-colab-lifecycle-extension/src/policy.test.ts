import { expect, it } from "vitest";
import { formatUsage, IDLE_MS, isColabModel, parseUsage, shouldStop } from "./policy.ts";

const base = { now: 10_000_000, lastRequestAt: 10_000_000 - IDLE_MS, running: true };

it("stops after the idle window", () => {
  expect(shouldStop(base)).toBe(true);
});

it("keeps running just before the window ends", () => {
  expect(shouldStop({ ...base, lastRequestAt: base.lastRequestAt + 1 })).toBe(false);
});

it("does nothing when already stopped", () => {
  expect(shouldStop({ ...base, running: false })).toBe(false);
});

it("matches only the google-colab provider", () => {
  expect(isColabModel({ provider: "google-colab" })).toBe(true);
  expect(isColabModel({ provider: "omlx" })).toBe(false);
  expect(isColabModel(undefined)).toBe(false);
});

it("parses colab usage output", () => {
  expect(
    parseUsage("Current balance: 194.09 compute units\nUsage rate: 5.38/hr\nActive assignments: 2"),
  ).toStrictEqual({
    balance: 194.09,
    ratePerHour: 5.38,
  });
  expect(parseUsage("garbage")).toBeUndefined();
  expect(parseUsage("Current balance: 1 compute units")).toBeUndefined();
});

it("formats the footer text with remaining hours", () => {
  expect(formatUsage({ balance: 100, ratePerHour: 5 })).toBe("Colab 100.0 CU · 5/hr · ~20.0h left");
  expect(formatUsage({ balance: 100, ratePerHour: 0 })).toBe("Colab 100.0 CU · 0/hr");
});
