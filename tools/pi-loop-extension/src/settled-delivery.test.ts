// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import { SettledDelivery } from "./settled-delivery.ts";

afterEach((): void => {
  vi.useRealTimers();
});

it("runs one coalesced callback on the next event-loop turn", () => {
  vi.useFakeTimers();
  const callback = vi.fn();
  const delivery = new SettledDelivery();

  delivery.schedule(callback);
  delivery.schedule(callback);
  expect(callback).not.toHaveBeenCalled();
  vi.runOnlyPendingTimers();

  expect(callback).toHaveBeenCalledOnce();
});

it("cancels a pending callback and permits a later schedule", () => {
  vi.useFakeTimers();
  const callback = vi.fn();
  const delivery = new SettledDelivery();
  delivery.cancel();
  delivery.schedule(callback);

  delivery.cancel();
  delivery.schedule(callback);
  vi.runOnlyPendingTimers();

  expect(callback).toHaveBeenCalledOnce();
});
