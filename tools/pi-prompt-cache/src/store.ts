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
import { parseLedger, type Ledger } from "./ledger.ts";

interface LedgerDocument extends JsonObject {
  state: string;
}

const DOCUMENT = defineDoc<LedgerDocument>({
  kind: "dotfiles.prompt-cache",
  version: 1,
  scope: "session",
  initial: () => ({ state: "" }),
});
const CONTEXT = BACKGROUND_CONTEXT;

/** The request ledger, committed to Pi Durable storage before the next request is measured. */
export class DurableLedgerStore {
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
  ): Promise<DurableLedgerStore> {
    const harness = await Harness.open(
      storage,
      { models: createModels(), registry: createRegistry(), onReport: report },
      CONTEXT,
    );
    return new DurableLedgerStore(harness, report);
  }

  async restore(): Promise<Ledger | undefined> {
    const doc = await this.#harness.snapshot(DOCUMENT, CONTEXT);
    if (doc === undefined || doc.state === "") {
      return undefined;
    }
    return parseLedger(doc.state);
  }

  save(ledger: Ledger): void {
    const serialized: string = JSON.stringify(ledger);
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
      // A save that lands after shutdown cannot be written anywhere; the session is already gone.
      return;
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
