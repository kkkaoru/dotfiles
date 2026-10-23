// Runs with bun: bundled by `bun run build`, tested by `bun run test`.
import {
  type FeatureId,
  type FeaturePatterns,
  JOIN_GRACE_MS,
  type MessageKey,
  type View,
  detectFeatures,
  isInCall,
  isMeetingPath,
  readPage,
  resolveView,
} from "../detection/state/meet-state.ts";
import { type Check, type Watcher, watchPage } from "../scheduling/page-watcher.ts";
import stylesheet from "./overlay.css";

export interface Overlay {
  readonly host: HTMLDivElement;
  readonly frame: HTMLDivElement;
  readonly label: HTMLDivElement;
}

export type Translate = (key: MessageKey) => string;

export interface RenderInput {
  readonly overlay: Overlay;
  readonly view: View;
  readonly translate: Translate;
}

export interface JoinState {
  hint: readonly FeatureId[];
  joinedAt: number;
}

export interface TickInput {
  readonly page: Document;
  readonly overlay: Overlay;
  readonly state: JoinState;
  readonly now: number;
  readonly translate: Translate;
  readonly patterns: FeaturePatterns;
}

export interface StartInput {
  readonly page: Document;
  readonly translate: Translate;
  readonly patterns: FeaturePatterns;
  // False once the extension is reloaded or updated: this content script then outlives its context.
  readonly isAlive: () => boolean;
}

// Lets a check stop the watcher that runs it; set once the watcher exists.
interface WatcherHandle {
  watcher: Watcher | undefined;
}

interface CheckerInput extends StartInput {
  readonly overlay: Overlay;
  readonly state: JoinState;
  readonly handle: WatcherHandle;
}

const NOT_JOINED = 0;
const LABEL_SEPARATOR = " + ";
const TONE_ATTRIBUTE = "tone";
const FRAME_CLASS = "frame";
const LABEL_CLASS = "label";

const classedDiv = (page: Document, className: string): HTMLDivElement => {
  const div = page.createElement("div");
  div.className = className;
  return div;
};

// A closed shadow root keeps Meet's CSS and our CSS from affecting each other.
export const createOverlay = (page: Document): Overlay => {
  const host = page.createElement("div");
  const style = page.createElement("style");
  const frame = classedDiv(page, FRAME_CLASS);
  const label = classedDiv(page, LABEL_CLASS);
  style.textContent = stylesheet;
  frame.hidden = true;
  frame.append(label);
  host.attachShadow({ mode: "closed" }).append(style, frame);
  return { host, frame, label };
};

// Writes only on change so an unchanged state costs no style recalculation.
export const renderView = ({ overlay, view, translate }: RenderInput): void => {
  const text = view.messages.map((key) => translate(key)).join(LABEL_SEPARATOR);
  overlay.frame.hidden = false;
  if (overlay.frame.dataset[TONE_ATTRIBUTE] !== view.tone) {
    overlay.frame.dataset[TONE_ATTRIBUTE] = view.tone;
  }
  if (overlay.label.textContent !== text) {
    overlay.label.textContent = text;
  }
};

const trackJoin = (input: TickInput, active: readonly FeatureId[], inCall: boolean): void => {
  // Before joining, the pre-join notice is the hint used during the join grace period.
  input.state.hint = inCall ? input.state.hint : active;
  input.state.joinedAt = inCall ? input.state.joinedAt || input.now : NOT_JOINED;
};

// Returns when to check again without a page change: the end of the join grace period.
export const tick = (input: TickInput): number | undefined => {
  if (!input.overlay.host.isConnected) {
    input.page.documentElement.append(input.overlay.host);
  }
  if (!isMeetingPath(input.page.location.pathname)) {
    input.overlay.frame.hidden = true;
    return undefined;
  }
  const text = readPage(input.page);
  const active = detectFeatures({ text, patterns: input.patterns });
  const inCall = isInCall(text);
  trackJoin(input, active, inCall);
  const msSinceJoin = input.now - input.state.joinedAt;
  renderView({
    overlay: input.overlay,
    view: resolveView({ active, hint: input.state.hint, inCall, msSinceJoin }),
    translate: input.translate,
  });
  return inCall && msSinceJoin < JOIN_GRACE_MS ? JOIN_GRACE_MS - msSinceJoin : undefined;
};

// An orphaned script stops watching and removes its border; the reloaded tab's script takes over.
const retire = (overlay: Overlay, handle: WatcherHandle): undefined => {
  handle.watcher?.stop();
  overlay.host.remove();
  return undefined;
};

// The watcher's check: render while the extension is alive, retire once it is not.
const checkerFor =
  (input: CheckerInput): Check =>
  (): number | undefined =>
    input.isAlive()
      ? tick({
          page: input.page,
          overlay: input.overlay,
          state: input.state,
          now: Date.now(),
          translate: input.translate,
          patterns: input.patterns,
        })
      : retire(input.overlay, input.handle);

export const start = (startInput: StartInput): Watcher => {
  const input: CheckerInput = {
    ...startInput,
    overlay: createOverlay(startInput.page),
    state: { hint: [], joinedAt: NOT_JOINED },
    handle: { watcher: undefined },
  };
  const watcher = watchPage({ page: input.page, check: checkerFor(input) });
  input.handle.watcher = watcher;
  return watcher;
};
