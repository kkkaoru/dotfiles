// Runs with bun: bundled by `bun run build`, tested by `bun run test`.
// Meet has no recording API; its indicators only expose localized labels and text.
// This module stays language-neutral: callers supply the phrases from src/detection/phrases/<lang>.json.

export type FeatureId = "recording" | "transcription" | "gemini";

// Keys of _locales/*/messages.json used for the on-screen label; also the overlay's CSS tone.
export type MessageKey = FeatureId | "loading" | "idle";

// Regular-expression sources per feature, as stored in src/detection/phrases/<lang>.json.
export type PhraseSet = Readonly<Record<FeatureId, readonly string[]>>;

export type FeaturePatterns = Readonly<Record<FeatureId, RegExp>>;

export interface DetectInput {
  readonly text: string;
  readonly patterns: FeaturePatterns;
}

export interface View {
  readonly tone: MessageKey;
  readonly messages: readonly MessageKey[];
}

export interface ViewInput {
  readonly active: readonly FeatureId[];
  readonly hint: readonly FeatureId[];
  readonly inCall: boolean;
  readonly msSinceJoin: number;
}

// Ordered by priority: the first active feature sets the tone, all of them go in the label.
export const FEATURES: readonly FeatureId[] = ["recording", "transcription", "gemini"];

const LOADING_VIEW: View = { tone: "loading", messages: ["loading"] };
const IDLE_VIEW: View = { tone: "idle", messages: ["idle"] };
// The leave-call button's Material Symbols ligature is a line of its own in every UI language.
const LEAVE_CALL_LINE = /^call_end$/mu;
// Meet renders in-call indicators a few seconds after joining; trust the pre-join notice meanwhile.
export const JOIN_GRACE_MS = 10_000;
const MEETING_PATH = /[a-z]{3}-[a-z]{4}-[a-z]{3}/u;
const LABELLED_SELECTOR = "[aria-label], [data-tooltip]";
const PHRASE_SEPARATOR = "|";
// The m flag lets anchored phrases like ^rec$ match one line of the combined page text.
const PHRASE_FLAGS = "imu";
const LINE_BREAK = "\n";

function patternOf(sources: readonly string[]): RegExp {
  return new RegExp(sources.join(PHRASE_SEPARATOR), PHRASE_FLAGS);
}

// Meet's UI language can differ from the browser's, so every language's phrases are combined.
export function compilePatterns(sets: readonly PhraseSet[]): FeaturePatterns {
  return {
    recording: patternOf(sets.flatMap((set) => set.recording)),
    transcription: patternOf(sets.flatMap((set) => set.transcription)),
    gemini: patternOf(sets.flatMap((set) => set.gemini)),
  };
}

export function isMeetingPath(pathname: string): boolean {
  return MEETING_PATH.test(pathname);
}

export function isInCall(text: string): boolean {
  return LEAVE_CALL_LINE.test(text);
}

function tooltipOf(element: Element): string | undefined {
  return element instanceof HTMLElement ? element.dataset["tooltip"] : undefined;
}

// Both are read: an icon can carry a generic aria-label and the indicator text in its tooltip.
function labelText(element: Element): string {
  return `${element.getAttribute("aria-label") ?? ""}${LINE_BREAK}${tooltipOf(element) ?? ""}`;
}

// One string per check, so each pattern scans the page once instead of once per line.
// The innerText follows the rendered layout, so a badge is its own line.
// It also skips hidden, script and style text (~74 KB on Meet) that textContent would include.
export function readPage(page: Document): string {
  return [
    ...[...page.querySelectorAll(LABELLED_SELECTOR)].map((element) => labelText(element)),
    // oxlint-disable-next-line unicorn/prefer-dom-node-text-content -- see the reason above.
    page.body.innerText,
  ].join(LINE_BREAK);
}

export function detectFeatures({ text, patterns }: DetectInput): FeatureId[] {
  return FEATURES.filter((feature) => patterns[feature].test(text));
}

function withGraceHint(input: ViewInput): FeatureId[] {
  return input.msSinceJoin < JOIN_GRACE_MS
    ? FEATURES.filter((feature) => input.active.includes(feature) || input.hint.includes(feature))
    : [...input.active];
}

function viewOf([first, ...rest]: readonly FeatureId[]): View {
  return first === undefined ? IDLE_VIEW : { tone: first, messages: [first, ...rest] };
}

export function resolveView(input: ViewInput): View {
  return input.inCall ? viewOf(withGraceHint(input)) : LOADING_VIEW;
}
