// This TypeScript file is executed with Bun.
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai";
import {
  createRegistry,
  defineDoc,
  defineExtension,
  defineTask,
  Harness,
  type Storage,
  type JsonObject,
  type TaskId,
} from "@earendil-works/pi-durable";
import type { Scheduler } from "./scheduler.ts";
import { latestLoopState, LOOP_STATE_ENTRY_TYPE, type LoopRuntimeState } from "./state.ts";

interface LoopDocument extends JsonObject {
  state: string;
  timer: TaskId<null> | null;
}
interface WaitCheckpoint {
  readonly phase: "wait";
  readonly due: number;
}
const DOCUMENT = defineDoc<LoopDocument>({
  kind: "dotfiles.loop",
  version: 1,
  scope: "session",
  initial: () => ({ state: "", timer: null }),
});
const CONTEXT = BACKGROUND_CONTEXT;

/** Durable state and a checkpointed waiting task; model/tool execution stays in Pi. */
export class DurableLoopStore {
  readonly #harness: Harness;
  readonly #report: (error: unknown) => void;
  #pending: Promise<void> = Promise.resolve();
  #failure: Error | undefined;
  #callback: (() => void) | undefined;
  #epoch = 0;
  #closed = false;

  private constructor(harness: Harness, report: (error: unknown) => void) {
    this.#harness = harness;
    this.#report = report;
  }

  static async open(storage: Storage, report: (error: unknown) => void): Promise<DurableLoopStore> {
    const registry = createRegistry();
    const failures = { report };
    const harness = await Harness.open(
      storage,
      { models: createModels(), registry, onReport: (error) => failures.report(error) },
      CONTEXT,
    );
    const store = new DurableLoopStore(harness, report);
    failures.report = (error): void => store.fail(error);
    const timer = defineTask<number, WaitCheckpoint, null>({
      name: "dotfiles.loop.wait",
      version: 1,
      initial: (interval) => ({ phase: "wait", due: Date.now() + interval }),
      phases: {
        wait: async (task, runtime, context) => {
          await runtime.sleep(task.state.checkpoint.due, context);
          store.#callback?.();
          await store.flush();
          await runtime.commit(async (tx) => {
            if (store.#callback !== undefined) {
              return {
                status: "running",
                checkpoint: { phase: "wait", due: Date.now() + task.input },
              };
            }
            const doc = await tx.doc(DOCUMENT);
            doc.timer = null;
            return { status: "terminal", outcome: { status: "completed", result: null } };
          }, context);
        },
      },
      abort: async (_task, runtime, context) => {
        await runtime.commit(async (tx) => {
          const doc = await tx.doc(DOCUMENT);
          doc.timer = null;
          return { status: "terminal", outcome: { status: "aborted" } };
        }, context);
      },
    });
    registry.install(defineExtension({ name: "dotfiles-loop", tasks: [timer] }));
    store.scheduler = {
      now: (): number => Date.now(),
      clearInterval: (cancel): void => cancel(),
      setInterval: (callback, interval) => {
        store.#callback = callback;
        store.#enqueue(async () => {
          const root = await harness.root(CONTEXT);
          await root.commit(async (tx) => {
            const doc = await tx.doc(DOCUMENT);
            const prior = doc.timer === null ? undefined : await tx.task(doc.timer);
            if (prior === undefined || prior.state.status === "terminal") {
              doc.timer = await tx.createTask(timer, interval, {
                ownership: { kind: "conversation" },
              });
            }
          }, CONTEXT);
          harness.resume();
        });
        return (): void => {
          if (store.#callback === callback) {
            store.#callback = undefined;
          }
        };
      },
    };
    return store;
  }

  // Assigned during open, before the store is returned.
  scheduler: Scheduler | undefined;

  async restore(legacy: LoopRuntimeState | undefined): Promise<LoopRuntimeState | undefined> {
    const doc = await this.#harness.snapshot(DOCUMENT, CONTEXT);
    if (doc === undefined || doc.state === "") {
      return legacy;
    }
    const data: unknown = JSON.parse(doc.state);
    const state = latestLoopState([{ type: "custom", customType: LOOP_STATE_ENTRY_TYPE, data }]);
    if (state === undefined) {
      throw new Error("Invalid Pi Durable loop state; refusing legacy fallback.");
    }
    return state;
  }

  save(state: LoopRuntimeState, afterCommit?: () => void): void {
    // Serialize now: the runtime's pending array is mutable.
    const serialized: string = JSON.stringify(state);
    this.#enqueue(async () => {
      await this.#harness.commit(async (tx) => {
        const doc = await tx.doc(DOCUMENT);
        doc.state = serialized;
      }, CONTEXT);
      afterCommit?.();
    });
  }

  cancelDelivery(): void {
    this.#epoch += 1;
  }

  deliver(send: () => void): void {
    const epoch: number = this.#epoch;
    this.#enqueue(async () => {
      if (epoch === this.#epoch && !this.#closed) {
        send();
      }
    });
  }

  fail(error: unknown): void {
    this.#failure = error instanceof Error ? error : new Error(String(error));
    this.#callback = undefined;
    this.#report(error);
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
    this.#callback = undefined;
    try {
      await this.flush();
    } finally {
      await this.#harness.close(CONTEXT);
    }
  }

  #enqueue(change: () => Promise<void>): void {
    if (this.#closed) {
      throw new Error("Pi Durable loop store is closed.");
    }
    this.#pending = this.#pending
      .then(async () => {
        if (this.#failure === undefined) {
          await change();
        }
      })
      .catch((error: unknown) => {
        this.fail(error);
      });
  }
}
