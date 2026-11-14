// This TypeScript file is executed with Bun.
import { setTimeout as delay } from "node:timers/promises";
import type { Component } from "@earendil-works/pi-tui";
import type {
  BeforeAgentStartEvent,
  InputEvent,
  InputEventResult,
  MarkdownTransformContext,
} from "@earendil-works/pi-coding-agent";
import type { Translator } from "../../claude-plamo-translate/hooks/translate.ts";
import { translateFast } from "./fast.ts";
import {
  assistantTexts,
  needsDisplayTranslation,
  OFF,
  remember,
  replyRule,
  SECTION_NAME,
  shouldTranslatePrompt,
  STATUS_KEY,
  statusText,
  transformedInput,
  WIDGET_KEY,
} from "./policy.ts";
import { SETUP_HINT, type Server } from "./server.ts";
import type { PlamoSettings } from "./settings.ts";

export interface RenderHost {
  invalidate(): void;
  requestRender(): void;
}

export interface UiSink {
  notify(message: string, type?: "info" | "warning" | "error"): void;
  setStatus(key: string, text: string | undefined): void;
  setWidget(
    key: string,
    content: (tui: RenderHost) => Component,
    options: { readonly placement: "belowEditor" },
  ): void;
}

interface Progress {
  done: number;
  total: number;
  // Whether the progress line was shown, and whether its timer loop is running.
  shown: boolean;
  looping: boolean;
}

export interface Runtime {
  readonly ready: Map<string, string>;
  readonly pending: Map<string, Promise<string>>;
  readonly typed: Map<string, string>;
  readonly server: Server;
  readonly settings: PlamoSettings;
  readonly sleep: (ms: number) => Promise<void>;
  host: RenderHost | null;
  progress: Progress;
  ui: UiSink;
  warned: boolean;
}

// A translation still running after the first delay reports itself, then once per interval.
const PROGRESS_DELAY_MS = 2000;
const PROGRESS_INTERVAL_MS = 5000;

// An unref'd timer never keeps pi alive while it waits.
const realSleep = async (ms: number): Promise<void> => {
  await delay(ms, undefined, { ref: false });
};

const silent: UiSink = {
  notify(): void {},
  setStatus(): void {},
  setWidget(): void {},
};

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "unknown error";
}

export function createRuntime(
  settings: PlamoSettings,
  server: Server,
  sleep: (ms: number) => Promise<void> = realSleep,
): Runtime {
  return {
    ready: new Map(),
    pending: new Map(),
    typed: new Map(),
    server,
    settings,
    sleep,
    host: null,
    progress: { done: 0, total: 0, shown: false, looping: false },
    ui: silent,
    warned: false,
  };
}

export function hiddenWidget(): Component {
  return {
    invalidate(): void {},
    render(_width: number): string[] {
      return [];
    },
  };
}

export function holdHost(runtime: Runtime, tui: RenderHost): Component {
  runtime.host = tui;
  return hiddenWidget();
}

function warnOnce(runtime: Runtime, message: string): void {
  if (runtime.warned) {
    return;
  }
  runtime.warned = true;
  runtime.ui.notify(message, "warning");
}

function redraw(host: RenderHost): void {
  host.invalidate();
  host.requestRender();
}

function showProgress(runtime: Runtime): void {
  runtime.ui.setStatus(STATUS_KEY, statusText(runtime.progress.done, runtime.progress.total));
}

function resetProgress(runtime: Runtime): void {
  // `looping` stays set: its timer exits the run itself, so a new run reuses it.
  runtime.progress = { ...runtime.progress, done: 0, total: 0, shown: false };
  runtime.ui.setStatus(STATUS_KEY, undefined);
}

// The progress line is pi's chat status line: notifying again rewrites the same line.
function showToast(runtime: Runtime): void {
  runtime.progress.shown = true;
  runtime.ui.notify(statusText(runtime.progress.done, runtime.progress.total));
}

function clearToast(runtime: Runtime): void {
  if (!runtime.progress.shown) {
    return;
  }
  runtime.progress.shown = false;
  runtime.ui.notify("");
}

// Recursion, as the Claude mod waits: one report per interval until the run ends.
async function reportWhileRunning(runtime: Runtime): Promise<void> {
  if (runtime.progress.total === 0) {
    runtime.progress.looping = false;
    clearToast(runtime);
    return;
  }
  showToast(runtime);
  await runtime.sleep(PROGRESS_INTERVAL_MS);
  return reportWhileRunning(runtime);
}

async function progressLoop(runtime: Runtime): Promise<void> {
  await runtime.sleep(PROGRESS_DELAY_MS);
  return reportWhileRunning(runtime);
}

function watchProgress(runtime: Runtime): void {
  if (runtime.progress.looping) {
    return;
  }
  runtime.progress.looping = true;
  progressLoop(runtime).catch((error: unknown) => {
    warnOnce(runtime, `plamo-translate: showing progress failed: ${errorMessage(error)}`);
  });
}

function begin(runtime: Runtime): void {
  runtime.progress.total += 1;
  showProgress(runtime);
  watchProgress(runtime);
}

function noteDone(runtime: Runtime): void {
  runtime.progress.done += 1;
  if (runtime.progress.done >= runtime.progress.total) {
    clearToast(runtime);
    resetProgress(runtime);
    return;
  }
  showProgress(runtime);
  if (runtime.progress.shown) {
    showToast(runtime);
  }
}

// One retry, because a dropped connection would leave the prompt in Japanese.
async function retriedChunk(runtime: Runtime, text: string, lang: string): Promise<string | null> {
  try {
    return await runtime.server.complete(text, lang);
  } catch (error: unknown) {
    warnOnce(runtime, `plamo-translate: a translation request failed: ${errorMessage(error)}`);
    return null;
  }
}

async function chunkOrNull(runtime: Runtime, text: string, lang: string): Promise<string | null> {
  try {
    return await runtime.server.complete(text, lang);
  } catch {
    return retriedChunk(runtime, text, lang);
  }
}

export async function translateText(runtime: Runtime, text: string, lang: string): Promise<string> {
  if (!(await runtime.server.ensure())) {
    warnOnce(runtime, SETUP_HINT);
    return text;
  }
  const translate: Translator = async (chunk) => chunkOrNull(runtime, chunk, lang);
  return translateFast(text, translate);
}

function paint(runtime: Runtime): void {
  if (runtime.host !== null) {
    redraw(runtime.host);
    return;
  }
  runtime.ui.setWidget(WIDGET_KEY, (tui) => holdHost(runtime, tui), {
    placement: "belowEditor",
  });
}

function finish(runtime: Runtime, source: string): void {
  runtime.pending.delete(source);
  noteDone(runtime);
  paint(runtime);
}

async function storeTranslation(runtime: Runtime, source: string, lang: string): Promise<string> {
  try {
    const stored = remember(runtime.ready, source, await translateText(runtime, source, lang));
    finish(runtime, source);
    return stored;
  } catch (error: unknown) {
    warnOnce(runtime, `plamo-translate: settling a translation failed: ${errorMessage(error)}`);
    finish(runtime, source);
    return source;
  }
}

export function schedule(runtime: Runtime, source: string, lang: string): void {
  if (runtime.pending.has(source) || runtime.ready.has(source)) {
    return;
  }
  // oxlint-disable-next-line typescript/no-floating-promises -- kept in pending; storeTranslation reports failure
  remember(runtime.pending, source, storeTranslation(runtime, source, lang));
  begin(runtime);
}

export function displayOf(
  runtime: Runtime,
  markdown: string,
  context: MarkdownTransformContext,
): string {
  if (!needsDisplayTranslation(markdown, runtime.settings.replyLanguage, context.isStreaming)) {
    return markdown;
  }
  const typed = runtime.typed.get(markdown);
  if (typed !== undefined) {
    return typed;
  }
  const ready = runtime.ready.get(markdown);
  if (ready !== undefined) {
    return ready;
  }
  schedule(runtime, markdown, runtime.settings.replyLanguage);
  return markdown;
}

export async function translateInput(
  runtime: Runtime,
  event: InputEvent,
  ui: UiSink,
): Promise<InputEventResult> {
  runtime.ui = ui;
  if (!shouldTranslatePrompt(event.text, event.source, runtime.settings.promptLanguage)) {
    return { action: "continue" };
  }
  begin(runtime);
  const english = await translateText(runtime, event.text, runtime.settings.promptLanguage);
  noteDone(runtime);
  if (english === event.text) {
    return { action: "continue" };
  }
  remember(runtime.typed, english, event.text);
  return transformedInput(english, event.images);
}

export function installRule(
  runtime: Runtime,
  sections: BeforeAgentStartEvent["systemPromptOptions"]["sections"],
): void {
  const rule = replyRule(runtime.settings.replyLanguage, runtime.settings.promptLanguage);
  if (rule === undefined) {
    return;
  }
  sections[SECTION_NAME] = rule;
}

export function scheduleReply(
  runtime: Runtime,
  message: { readonly content?: unknown; readonly role: string },
): void {
  if (runtime.settings.replyLanguage === OFF) {
    return;
  }
  assistantTexts(message).forEach((text) => {
    schedule(runtime, text, runtime.settings.replyLanguage);
  });
}

export function startSession(runtime: Runtime, ui: UiSink): void {
  runtime.ui = ui;
  ui.setWidget(WIDGET_KEY, (tui) => holdHost(runtime, tui), { placement: "belowEditor" });
  runtime.server.ensure().catch((error: unknown) => {
    warnOnce(runtime, `plamo-translate: warming the server failed: ${errorMessage(error)}`);
  });
}
