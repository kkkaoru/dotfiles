import { MemoryStorage, type Storage } from "@earendil-works/pi-durable";
import { describe, expect, it, vi } from "vitest";
import { addNode, EMPTY, link, type Graph } from "./graph.ts";
import { DurableRelationsStore } from "./store.ts";

const SESSION = "session:01a1fcbf";
const DOC = "doc:docs/report.md";

function graph(): Graph {
  return link(addNode(EMPTY, SESSION, "panic investigation"), {
    from: SESSION,
    relation: "produced",
    to: DOC,
  });
}

async function open(storage: Storage = new MemoryStorage()): Promise<{
  report: ReturnType<typeof vi.fn<(error: unknown) => void>>;
  store: DurableRelationsStore;
}> {
  const report = vi.fn<(error: unknown) => void>();
  const store = await DurableRelationsStore.open(storage, report);
  return { report, store };
}

/** A backend that refuses every commit, which is what a full disk looks like. */
function failing(storage: Storage): Storage {
  return new Proxy(storage, {
    get(target, property, receiver): unknown {
      if (property === "commit") {
        return async (): Promise<never> => {
          throw new Error("disk full");
        };
      }
      return Reflect.get(target, property, receiver) as unknown;
    },
  });
}

describe("DurableRelationsStore", () => {
  it("has nothing to restore on a fresh store", async () => {
    const { store } = await open();
    await expect(store.restore()).resolves.toBeUndefined();
    await store.close();
  });

  it("keeps the graph across a save and a restore", async () => {
    const { store } = await open();
    store.save(graph());
    await store.flush();
    await expect(store.restore()).resolves.toEqual(graph());
    await store.close();
  });

  it("refuses to save after close", async () => {
    const { store } = await open();
    await store.close();
    expect(() => store.save(graph())).toThrow(/closed/);
  });

  it("closes twice without complaint", async () => {
    const { store } = await open();
    await store.close();
    await expect(store.close()).resolves.toBeUndefined();
  });

  it("reports a failed commit and fails the flush", async () => {
    const { report, store } = await open(failing(new MemoryStorage()));
    store.save(graph());
    await expect(store.flush()).rejects.toThrow(/disk full/);
    expect(report).toHaveBeenCalled();
    // The failure was already reported, so closing must not throw it again.
    await expect(store.close()).resolves.toBeUndefined();
  });

  it("reports a failure that is not an Error", async () => {
    const throwing = new Proxy(new MemoryStorage(), {
      get(target, property, receiver): unknown {
        if (property === "commit") {
          return async (): Promise<never> => {
            // A driver may reject with anything, which is why the store guards it.
            throw "disk gone";
          };
        }
        return Reflect.get(target, property, receiver) as unknown;
      },
    });
    const { store } = await open(throwing);
    store.save(graph());
    await expect(store.flush()).rejects.toThrow(/disk gone/u);
    await store.close();
  });

  it("drops a save queued after a failure", async () => {
    const { store } = await open(failing(new MemoryStorage()));
    store.save(graph());
    store.save(graph());
    await expect(store.flush()).rejects.toThrow(/disk full/);
    await store.close();
  });
});
