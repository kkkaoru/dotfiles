// This TypeScript file is executed with Bun.
import { createHash } from "node:crypto";
import { mkdir } from "node:fs/promises";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels, type Usage } from "@earendil-works/pi-ai";
import {
  createRegistry,
  defineDoc,
  Harness,
  type JsonObject,
  type Storage,
} from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import {
  summarizeBounded,
  type SummaryCheckpoint,
  type SummaryCheckpointStore,
  type SummaryRequest,
  type SummaryResult,
} from "./bounded-summary.ts";
import { isRecord } from "./settings.ts";

interface SummaryDocument extends JsonObject {
  key: string;
  checkpoint: string;
}
export interface DurableSummaryRequest extends SummaryRequest {
  readonly sessionFile: string | undefined;
  readonly identity: string;
}
const DOCUMENT = defineDoc<SummaryDocument>({
  kind: "dotfiles.effort.compaction",
  version: 1,
  scope: "session",
  initial: () => ({ key: "", checkpoint: "" }),
});
const USAGE_KEYS: readonly string[] = ["input", "output", "cacheRead", "cacheWrite", "totalTokens"];
const COST_KEYS: readonly string[] = ["input", "output", "cacheRead", "cacheWrite", "total"];
// Bump when summary prompts, segmentation, output caps or reasoning policy change.
const CHECKPOINT_VERSION = 1 satisfies number;

function validNumber(value: unknown): boolean {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}
function validUsage(value: unknown): value is Usage {
  if (!isRecord(value) || !isRecord(value["cost"])) {
    return false;
  }
  const cost: Record<string, unknown> = value["cost"];
  return (
    USAGE_KEYS.every((key) => validNumber(value[key])) &&
    COST_KEYS.every((key) => validNumber(cost[key])) &&
    (value["reasoning"] === undefined || validNumber(value["reasoning"]))
  );
}
function decodeCheckpoint(serialized: string): SummaryCheckpoint {
  const value: unknown = JSON.parse(serialized);
  if (
    !isRecord(value) ||
    typeof value["text"] !== "string" ||
    typeof value["nextChunk"] !== "number" ||
    !validUsage(value["usage"])
  ) {
    throw new Error("Invalid Pi Durable compaction checkpoint");
  }
  return { text: value["text"], nextChunk: value["nextChunk"], usage: value["usage"] };
}

/** Only checkpoints are durable here; Pi still owns model calls and cancellation. */
export class DurableSummaryStore implements SummaryCheckpointStore {
  readonly #harness: Harness;
  readonly #key: string;

  private constructor(harness: Harness, key: string) {
    this.#harness = harness;
    this.#key = key;
  }

  static async open(storage: Storage, key: string): Promise<DurableSummaryStore> {
    const harness: Harness = await Harness.open(
      storage,
      {
        models: createModels(),
        registry: createRegistry(),
      },
      BACKGROUND_CONTEXT,
    );
    return new DurableSummaryStore(harness, key);
  }

  async load(): Promise<SummaryCheckpoint | undefined> {
    const doc: SummaryDocument | undefined = await this.#harness.snapshot(
      DOCUMENT,
      BACKGROUND_CONTEXT,
    );
    return doc === undefined || doc.key !== this.#key || doc.checkpoint === ""
      ? undefined
      : decodeCheckpoint(doc.checkpoint);
  }

  async save(checkpoint: SummaryCheckpoint): Promise<void> {
    const serialized: string = JSON.stringify(checkpoint);
    await this.#harness.commit(async (tx) => {
      const doc: SummaryDocument = await tx.doc(DOCUMENT);
      doc.key = this.#key;
      doc.checkpoint = serialized;
    }, BACKGROUND_CONTEXT);
  }

  async close(): Promise<void> {
    await this.#harness.close(BACKGROUND_CONTEXT);
  }
}

export async function summarizeDurably(request: DurableSummaryRequest): Promise<SummaryResult> {
  request.signal.throwIfAborted();
  if (request.sessionFile === undefined) {
    return summarizeBounded(request);
  }
  const key = createHash("sha256")
    .update(
      JSON.stringify({
        version: CHECKPOINT_VERSION,
        identity: request.identity,
        text: request.text,
        contextWindow: request.contextWindow,
      }),
    )
    .digest("hex");
  const directory = `${request.sessionFile}.effort-durable` satisfies string;
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const failure: { error: Error | undefined } = { error: undefined };
  const release: () => Promise<void> = await lock(directory, {
    retries: 0,
    onCompromised: (error): void => {
      failure.error = error;
    },
  });
  try {
    const storage: Storage = await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, {
      fsync: true,
    });
    const store: DurableSummaryStore = await DurableSummaryStore.open(storage, key);
    try {
      const checkLock = (): void => {
        if (failure.error !== undefined) {
          throw failure.error;
        }
      };
      const result: SummaryResult = await summarizeBounded({
        ...request,
        complete: async (prompt) => {
          checkLock();
          return request.complete(prompt);
        },
        checkpoint: {
          load: async () => {
            checkLock();
            return store.load();
          },
          save: async (checkpoint) => {
            checkLock();
            await store.save(checkpoint);
          },
        },
      });
      checkLock();
      return result;
    } finally {
      await store.close();
    }
  } finally {
    await release();
  }
}
