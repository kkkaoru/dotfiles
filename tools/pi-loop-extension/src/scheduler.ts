// This TypeScript file is executed with Bun.
import { clearInterval, setInterval } from "node:timers";

export type Poller = () => void;

export interface Scheduler {
  readonly clearInterval: (poller: Poller) => void;
  readonly now: () => number;
  readonly setInterval: (callback: () => void, intervalMs: number) => Poller;
}

// Used by isolated runtime tests; the installed extension supplies Pi Durable.
export const SYSTEM_SCHEDULER: Scheduler = {
  clearInterval: (poller): void => poller(),
  now: (): number => Date.now(),
  setInterval: (callback, intervalMs): Poller => {
    const timer = setInterval(callback, intervalMs);
    return (): void => clearInterval(timer);
  },
};
