import { clearInterval, setInterval } from "node:timers";
import type { RepeatScheduler } from "./contracts.ts";

export const MONITOR_INTERVAL_MS = 5000 satisfies number;

export const SYSTEM_SCHEDULER: RepeatScheduler = {
  repeat(task: () => void, intervalMs: number): () => void {
    const timer: ReturnType<typeof setInterval> = setInterval(task, intervalMs);
    return (): void => clearInterval(timer);
  },
};
