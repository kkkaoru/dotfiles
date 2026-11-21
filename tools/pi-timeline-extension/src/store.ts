// This TypeScript file is executed with Bun.
import { readdir, stat } from "node:fs/promises";
import path from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai";
import {
  createRegistry,
  defineDoc,
  Harness,
  type JsonObject,
  type Storage,
} from "@earendil-works/pi-durable";
import { parseTimeline, type Timeline } from "./tree.ts";

interface TimelineDocument extends JsonObject {
  state: string;
}

const DOCUMENT = defineDoc<TimelineDocument>({
  kind: "dotfiles.timeline",
  version: 1,
  scope: "session",
  initial: () => ({ state: "" }),
});
const CONTEXT = BACKGROUND_CONTEXT;

/** The task tree, committed to Pi Durable storage before it is shown. */
export class DurableTimelineStore {
  readonly #harness: Harness;
  readonly #report: (error: unknown) => void;
  #pending: Promise<void> = Promise.resolve();
  #failure: Error | undefined;
  #closed = false;

  private constructor(harness: Harness, report: (error: unknown) => void) {
    this.#harness = harness;
    this.#report = report;
  }

  static async open(
    storage: Storage,
    report: (error: unknown) => void,
  ): Promise<DurableTimelineStore> {
    const harness = await Harness.open(
      storage,
      { models: createModels(), registry: createRegistry(), onReport: report },
      CONTEXT,
    );
    return new DurableTimelineStore(harness, report);
  }

  async restore(): Promise<Timeline | undefined> {
    const doc = await this.#harness.snapshot(DOCUMENT, CONTEXT);
    if (doc === undefined || doc.state === "") {
      return undefined;
    }
    return parseTimeline(doc.state);
  }

  save(timeline: Timeline): void {
    // Serialize now: the caller's object is mutable.
    const serialized: string = JSON.stringify(timeline);
    this.#enqueue(async () => {
      await this.#harness.commit(async (tx) => {
        const doc = await tx.doc(DOCUMENT);
        doc.state = serialized;
      }, CONTEXT);
    });
  }

  async flush(): Promise<void> {
    await this.#pending;
    if (this.#failure !== undefined) {
      throw this.#failure;
    }
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    try {
      // The failure was already reported; shutting down must not throw it again.
      await this.#pending;
    } finally {
      await this.#harness.close(CONTEXT);
    }
  }

  #enqueue(change: () => Promise<void>): void {
    if (this.#closed) {
      throw new Error("Pi Durable timeline store is closed.");
    }
    this.#pending = this.#pending
      .then(async () => {
        if (this.#failure === undefined) {
          await change();
        }
      })
      .catch((error: unknown) => {
        this.#failure = error instanceof Error ? error : new Error(String(error));
        this.#report(error);
      });
  }
}

/** The sidecar's newest write, which is when the tree last changed. */
export async function lastWrite(directory: string): Promise<number | undefined> {
  const names = await readdir(directory);
  const written = names.filter((name) => name.endsWith(".jsonl"));
  const times = await Promise.all(
    written.map(async (name) => {
      const file = await stat(path.join(directory, name));
      return file.mtimeMs;
    }),
  );
  return times.length === 0 ? undefined : Math.max(...times);
}
