// This TypeScript file is executed with Bun.
import type { Task, Timeline } from "./tree.ts";

/** The newest translations kept; a task tree is small, so this only bounds a long session. */
export const CACHE_LIMIT = 500;

export interface Completer {
  /** Translate `text` into `lang`; null when the server answered nothing usable. */
  complete: (text: string, lang: string) => Promise<string | null>;
  /** Start the server when it is not running yet; absent when the completer needs no start. */
  ensure?: () => Promise<boolean>;
}

/**
 * The tree as the reader reads it: every title and note translated once and then kept, so a
 * repaint of an unchanged task costs nothing and only new or edited text reaches the server.
 */
export class TranslatedTimeline {
  readonly #cache = new Map<string, string>();
  readonly #complete: Completer;
  readonly #lang: string;

  constructor(lang: string, complete: Completer) {
    this.#lang = lang;
    this.#complete = complete;
  }

  async translate(timeline: Timeline): Promise<Timeline> {
    await this.#ready();
    const tasks = await Promise.all(timeline.tasks.map(async (task) => this.#task(task)));
    return { tasks };
  }

  /** A server that a restart left stopped is started here, once, before the first text. */
  async #ready(): Promise<void> {
    try {
      await this.#complete.ensure?.();
    } catch {
      // The texts below keep their English; the next paint starts the server again.
    }
  }

  async #task(task: Task): Promise<Task> {
    const title = await this.#one(task.title);
    const note = task.note === "" ? "" : await this.#one(task.note);
    return { ...task, note, title };
  }

  /** One text, translated once. A failed request is not kept, so a later paint retries it. */
  async #one(text: string): Promise<string> {
    const known = this.#cache.get(text);
    if (known !== undefined) {
      return known;
    }
    const result = await this.#ask(text);
    if (result === text) {
      return text;
    }
    if (this.#cache.size >= CACHE_LIMIT) {
      this.#cache.clear();
    }
    this.#cache.set(text, result);
    return result;
  }

  async #ask(text: string): Promise<string> {
    try {
      const translated = await this.#complete.complete(text, this.#lang);
      return translated === null || translated.trim() === "" ? text : translated.trim();
    } catch {
      return text;
    }
  }
}
