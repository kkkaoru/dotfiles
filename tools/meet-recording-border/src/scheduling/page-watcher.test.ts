// Runs with bun: `bun run test` (vitest + happy-dom).
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { watchPage } from "./page-watcher.ts";

interface ObserverRecord {
  readonly callback: MutationCallback;
  readonly observer: MutationObserver;
  readonly targets: Node[];
  readonly options: MutationObserverInit[];
  disconnected: boolean;
}

const idleOptions: IdleRequestOptions[] = [];
const observerRecords: ObserverRecord[] = [];

// Happy-dom delivers mutations on its own real timer, which races fake timers; tests notify by hand.
class FakeMutationObserver implements MutationObserver {
  readonly #record: ObserverRecord;

  constructor(callback: MutationCallback) {
    this.#record = { callback, observer: this, targets: [], options: [], disconnected: false };
    observerRecords.push(this.#record);
  }

  observe(target: Node, options: MutationObserverInit): void {
    this.#record.targets.push(target);
    this.#record.options.push(options);
    this.#record.disconnected = false;
  }

  disconnect(): void {
    this.#record.disconnected = true;
  }

  takeRecords(): MutationRecord[] {
    return [];
  }
}

function notifyMutation(): void {
  observerRecords
    .filter((record) => !record.disconnected)
    .map((record) => record.callback([], record.observer));
}

beforeEach(() => {
  vi.useFakeTimers();
  idleOptions.length = 0;
  observerRecords.length = 0;
  vi.stubGlobal("MutationObserver", FakeMutationObserver);
  // Happy-dom has no requestIdleCallback, so idle work runs as soon as it is requested.
  vi.stubGlobal(
    "requestIdleCallback",
    (callback: IdleRequestCallback, options: IdleRequestOptions): number => {
      idleOptions.push(options);
      callback({ didTimeout: false, timeRemaining: () => 50 });
      return 0;
    },
  );
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "hidden");
});

test("watchPage observes every kind of change below the document element after a check", () => {
  const watcher = watchPage({ page: document, check: () => undefined });
  expect(observerRecords[0]?.targets.length).toBe(0);
  vi.advanceTimersByTime(500);
  expect(observerRecords.length).toBe(1);
  expect(observerRecords[0]?.targets[0]).toBe(document.documentElement);
  expect(observerRecords[0]?.options).toStrictEqual([
    { attributes: true, characterData: true, childList: true, subtree: true },
  ]);
  watcher.stop();
});

test("watchPage runs the first check after the minimum gap, in idle time", () => {
  const check = vi.fn(() => undefined);
  const watcher = watchPage({ page: document, check });
  vi.advanceTimersByTime(499);
  expect(check.mock.calls.length).toBe(0);
  vi.advanceTimersByTime(1);
  expect(check.mock.calls.length).toBe(1);
  expect(idleOptions).toStrictEqual([{ timeout: 1000 }]);
  watcher.stop();
});

test("watchPage does nothing while the page stays unchanged", () => {
  const check = vi.fn(() => undefined);
  const watcher = watchPage({ page: document, check });
  vi.advanceTimersByTime(60_000);
  expect(check.mock.calls.length).toBe(1);
  watcher.stop();
});

test("watchPage coalesces a burst of changes into one check", () => {
  const check = vi.fn(() => undefined);
  const watcher = watchPage({ page: document, check });
  vi.advanceTimersByTime(500);
  notifyMutation();
  notifyMutation();
  vi.advanceTimersByTime(100);
  notifyMutation();
  vi.advanceTimersByTime(399);
  expect(check.mock.calls.length).toBe(1);
  vi.advanceTimersByTime(1);
  expect(check.mock.calls.length).toBe(2);
  watcher.stop();
});

test("watchPage checks again when a check asks for a later recheck", () => {
  const check = vi.fn<() => number | undefined>().mockReturnValueOnce(3000);
  const watcher = watchPage({ page: document, check });
  vi.advanceTimersByTime(500);
  vi.advanceTimersByTime(3499);
  expect(check.mock.calls.length).toBe(1);
  vi.advanceTimersByTime(1);
  expect(check.mock.calls.length).toBe(2);
  watcher.stop();
});

test("watchPage skips checks while hidden and checks again when the tab becomes visible", () => {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
  const check = vi.fn(() => undefined);
  const watcher = watchPage({ page: document, check });
  vi.advanceTimersByTime(500);
  expect(check.mock.calls.length).toBe(0);
  Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
  document.dispatchEvent(new Event("visibilitychange"));
  vi.advanceTimersByTime(500);
  expect(check.mock.calls.length).toBe(1);
  watcher.stop();
});

test("watchPage stops observing and drops a pending check after stop", () => {
  const check = vi.fn(() => undefined);
  const watcher = watchPage({ page: document, check });
  watcher.stop();
  vi.advanceTimersByTime(500);
  notifyMutation();
  document.dispatchEvent(new Event("visibilitychange"));
  vi.advanceTimersByTime(5000);
  expect(check.mock.calls.length).toBe(0);
  expect(observerRecords[0]?.disconnected).toBe(true);
});

test("watchPage detaches the observer while a check is pending", () => {
  const check = vi.fn(() => undefined);
  const watcher = watchPage({ page: document, check });
  vi.advanceTimersByTime(500);
  expect(observerRecords[0]?.disconnected).toBe(false);
  notifyMutation();
  expect(observerRecords[0]?.disconnected).toBe(true);
  notifyMutation();
  vi.advanceTimersByTime(500);
  expect(check.mock.calls.length).toBe(2);
  expect(observerRecords[0]?.disconnected).toBe(false);
  watcher.stop();
});

test("watchPage stays detached in a hidden tab", () => {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
  const watcher = watchPage({ page: document, check: () => undefined });
  vi.advanceTimersByTime(5000);
  expect(observerRecords[0]?.targets.length).toBe(0);
  expect(observerRecords[0]?.disconnected).toBe(true);
  watcher.stop();
});
