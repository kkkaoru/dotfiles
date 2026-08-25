// This TypeScript file is executed with Bun.
import { clearInterval, setInterval } from "node:timers";

export type Poller = ReturnType<typeof setInterval>;

export interface Scheduler {
  readonly clearInterval: (poller: Poller) => void;
  readonly now: () => number;
  readonly setInterval: (callback: () => void, intervalMs: number) => Poller;
}

export const SYSTEM_SCHEDULER: Scheduler = {
  clearInterval: (poller: Poller): void => clearInterval(poller),
  now: (): number => Date.now(),
  setInterval: (callback: () => void, intervalMs: number): Poller =>
    setInterval(callback, intervalMs),
};
