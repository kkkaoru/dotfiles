// Runs with bun: bundled by `bun run build`, tested by `bun run test`.
// Runs a check only after the page changes, at most once per MIN_GAP_MS and in idle time.
// The observer is detached while a check is pending or the tab is hidden.
// Meet mutates its DOM constantly during a call, and each observed change allocates a record.

// A check may ask to run again after this many milliseconds even if the page stays unchanged.
export type Check = () => number | undefined;

export interface WatchInput {
  readonly page: Document;
  readonly check: Check;
}

export interface Watcher {
  readonly stop: () => void;
}

interface WatchState {
  pending: boolean;
  stopped: boolean;
}

interface WatchContext {
  readonly page: Document;
  readonly check: Check;
  readonly state: WatchState;
  readonly observer: MutationObserver;
  readonly onChange: () => void;
}

type Reschedule = (context: WatchContext) => void;

const MIN_GAP_MS = 500;
const IDLE_TIMEOUT_MS = 1000;
const VISIBILITY_EVENT = "visibilitychange";
// Attributes are observed too: Meet can reveal an indicator by changing only class or style.
const OBSERVED_CHANGES: MutationObserverInit = {
  attributes: true,
  characterData: true,
  childList: true,
  subtree: true,
};

const observe = (context: WatchContext): void =>
  context.observer.observe(context.page.documentElement, OBSERVED_CHANGES);

// A hidden tab stays detached until visibilitychange schedules the next check.
const runCheck = (context: WatchContext, reschedule: Reschedule): void => {
  context.state.pending = false;
  if (context.state.stopped || context.page.hidden) {
    return;
  }
  // Reattach first so a throwing check cannot leave the page unwatched.
  observe(context);
  const recheckIn = context.check();
  if (recheckIn !== undefined) {
    globalThis.setTimeout(reschedule, recheckIn, context);
  }
};

const waitForIdle = (context: WatchContext, reschedule: Reschedule): void => {
  globalThis.requestIdleCallback(() => runCheck(context, reschedule), {
    timeout: IDLE_TIMEOUT_MS,
  });
};

// The pending check covers every later change, so nothing is observed until it has run.
const schedule = (context: WatchContext): void => {
  if (context.state.pending || context.state.stopped) {
    return;
  }
  context.state.pending = true;
  context.observer.disconnect();
  globalThis.setTimeout(waitForIdle, MIN_GAP_MS, context, schedule);
};

const stopWatching = (context: WatchContext): void => {
  context.state.stopped = true;
  context.observer.disconnect();
  context.page.removeEventListener(VISIBILITY_EVENT, context.onChange);
};

export const watchPage = ({ page, check }: WatchInput): Watcher => {
  const context: WatchContext = {
    page,
    check,
    state: { pending: false, stopped: false },
    observer: new MutationObserver(() => schedule(context)),
    onChange: () => schedule(context),
  };
  page.addEventListener(VISIBILITY_EVENT, context.onChange);
  schedule(context);
  return { stop: () => stopWatching(context) };
};
