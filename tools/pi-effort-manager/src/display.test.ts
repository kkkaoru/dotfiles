// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { NO_OBSERVATIONS, withObservation } from "./display.ts";

it("adds one reasoning observation to the running count, maximum and total", () => {
  expect(withObservation(NO_OBSERVATIONS, 120)).toStrictEqual({
    count: 1,
    maximum: 120,
    total: 120,
  });
  expect(withObservation({ count: 1, maximum: 120, total: 120 }, 40)).toStrictEqual({
    count: 2,
    maximum: 120,
    total: 160,
  });
});
