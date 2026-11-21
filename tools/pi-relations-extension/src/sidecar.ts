// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import { EMPTY, type Graph } from "./graph.ts";
import { DurableRelationsStore } from "./store.ts";

/** Beside the session file, like the loop, tmux and timeline sidecars, so a resumed session finds it. */
export const SIDECAR = ".relations-durable";

export interface OpenedRelations {
  readonly graph: Graph;
  readonly store: DurableRelationsStore;
  /** Close the store and drop the sidecar lock. Safe to call once. */
  readonly close: () => Promise<void>;
}

/** Open this session's graph. Without a session file the graph lives in memory only. */
export async function openRelations(
  file: string | undefined,
  report: (error: unknown) => void,
): Promise<OpenedRelations> {
  if (file === undefined) {
    const store = await DurableRelationsStore.open(new MemoryStorage(), report);
    return { graph: (await store.restore()) ?? EMPTY, store, close: async () => store.close() };
  }
  const directory = `${file}${SIDECAR}`;
  await mkdir(directory, { recursive: true, mode: 0o700 });
  // A tool call and the CLI may meet; the second waits for the first to close.
  const release = await lock(directory, { retries: 3 });
  try {
    const storage = await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, { fsync: true });
    const store = await DurableRelationsStore.open(storage, report);
    const graph = (await store.restore()) ?? EMPTY;
    return {
      graph,
      store,
      close: async () => {
        try {
          await store.close();
        } finally {
          await release();
        }
      },
    };
  } catch (error) {
    await release();
    throw error;
  }
}
