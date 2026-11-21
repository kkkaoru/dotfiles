// This TypeScript file is executed with Bun.
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai";
import {
  createRegistry,
  defineDoc,
  Harness,
  type JsonObject,
  type Storage,
} from "@earendil-works/pi-durable";
import { parseGraph, type Graph } from "./graph.ts";

interface RelationsDocument extends JsonObject {
  state: string;
}

const DOCUMENT = defineDoc<RelationsDocument>({
  kind: "dotfiles.relations",
  version: 1,
  scope: "session",
  initial: () => ({ state: "" }),
});
const CONTEXT = BACKGROUND_CONTEXT;

/** The relation graph, committed to Pi Durable storage before it is answered. */
export class DurableRelationsStore {
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
  ): Promise<DurableRelationsStore> {
    const harness = await Harness.open(
      storage,
      { models: createModels(), registry: createRegistry(), onReport: report },
      CONTEXT,
    );
    return new DurableRelationsStore(harness, report);
  }

  async restore(): Promise<Graph | undefined> {
    const doc = await this.#harness.snapshot(DOCUMENT, CONTEXT);
    if (doc === undefined || doc.state === "") {
      return undefined;
    }
    return parseGraph(JSON.parse(doc.state));
  }

  save(graph: Graph): void {
    // Serialize now: the caller's graph is immutable, but a later save must not race this one.
    const serialized: string = JSON.stringify(graph);
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
      throw new Error("Pi Durable relations store is closed.");
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
