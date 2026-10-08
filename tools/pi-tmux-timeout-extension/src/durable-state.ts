// This TypeScript file is executed with Bun.
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels } from "@earendil-works/pi-ai";
import {
  createRegistry,
  defineDoc,
  defineExtension,
  defineTask,
  Harness,
  type JsonObject,
  type Storage,
  type TaskId,
} from "@earendil-works/pi-durable";
import { ACTIVE_DISPLAY_ENTRY_TYPE } from "./active-display.ts";
import {
  TMUX_SESSION_ENTRY_TYPE,
  TMUX_DELIVERED_ENTRY_TYPE,
  TMUX_COMPLETED_ENTRY_TYPE,
} from "./persistence.ts";
import { RECONCILIATION_INTERVAL_MILLISECONDS } from "./tmux.ts";

interface Journal extends JsonObject {
  records: Record<string, string>;
  timer: TaskId<null> | null;
}
interface WaitCheckpoint {
  readonly phase: "wait";
  readonly due: number;
}
const CONTEXT = BACKGROUND_CONTEXT;
const JOURNAL = defineDoc<Journal>({
  kind: "dotfiles.tmux",
  version: 1,
  scope: "session",
  initial: () => ({ records: {}, timer: null }),
});

function entryKey(customType: string, data: unknown): string {
  if (customType === ACTIVE_DISPLAY_ENTRY_TYPE) {
    return customType;
  }
  if (
    (customType === TMUX_SESSION_ENTRY_TYPE ||
      customType === TMUX_DELIVERED_ENTRY_TYPE ||
      customType === TMUX_COMPLETED_ENTRY_TYPE) &&
    typeof data === "object" &&
    data !== null &&
    "sessionName" in data &&
    typeof data.sessionName === "string"
  ) {
    return `${customType}:${data.sessionName}`;
  }
  throw new Error("Invalid tmux journal entry");
}

/** Durable bookkeeping never executes or replays a shell command. */
export class DurableTmuxState {
  readonly #harness: Harness;
  readonly #report: (error: unknown) => void;
  #pending: Promise<void> = Promise.resolve();
  #failure: Error | undefined;
  #closed = false;
  #callback: (() => void) | undefined;
  monitor: ((callback: () => void) => () => void) | undefined;

  private constructor(harness: Harness, report: (error: unknown) => void) {
    this.#harness = harness;
    this.#report = report;
  }

  static async open(storage: Storage, report: (error: unknown) => void): Promise<DurableTmuxState> {
    const registry = createRegistry();
    const failures = { report };
    const harness = await Harness.open(
      storage,
      { registry, models: createModels(), onReport: (error) => failures.report(error) },
      CONTEXT,
    );
    const state = new DurableTmuxState(harness, report);
    failures.report = (error): void => state.fail(error);
    const task = defineTask<null, WaitCheckpoint, null>({
      name: "dotfiles.tmux.monitor",
      version: 1,
      initial: () => ({ phase: "wait", due: Date.now() + RECONCILIATION_INTERVAL_MILLISECONDS }),
      phases: {
        wait: async (current, runtime, context) => {
          await runtime.sleep(current.state.checkpoint.due, context);
          state.#callback?.();
          await state.flush();
          await runtime.commit(async (tx) => {
            if (state.#callback !== undefined) {
              return {
                status: "running",
                checkpoint: {
                  phase: "wait",
                  due: Date.now() + RECONCILIATION_INTERVAL_MILLISECONDS,
                },
              };
            }
            const doc = await tx.doc(JOURNAL);
            doc.timer = null;
            return { status: "terminal", outcome: { status: "completed", result: null } };
          }, context);
        },
      },
      abort: async (_current, runtime, context) => {
        await runtime.commit(async (tx) => {
          const doc = await tx.doc(JOURNAL);
          doc.timer = null;
          return { status: "terminal", outcome: { status: "aborted" } };
        }, context);
      },
    });
    registry.install(defineExtension({ name: "dotfiles-tmux", tasks: [task] }));
    state.monitor = (callback) => {
      state.#callback = callback;
      state.#enqueue(async () => {
        const root = await harness.root(CONTEXT);
        await root.commit(async (tx) => {
          const doc = await tx.doc(JOURNAL);
          const prior = doc.timer === null ? undefined : await tx.task(doc.timer);
          if (prior === undefined || prior.state.status === "terminal") {
            doc.timer = await tx.createTask(task, null, { ownership: { kind: "conversation" } });
          }
        }, CONTEXT);
        harness.resume();
      });
      return (): void => {
        if (state.#callback === callback) {
          state.#callback = undefined;
        }
      };
    };
    return state;
  }

  async restore(legacy: readonly unknown[]): Promise<readonly unknown[]> {
    const doc = await this.#harness.snapshot(JOURNAL, CONTEXT);
    if (doc !== undefined) {
      return Object.values(doc.records).map((text): unknown => JSON.parse(text));
    }
    legacy.map((entry): undefined => {
      if (
        typeof entry !== "object" ||
        entry === null ||
        !("customType" in entry) ||
        !("data" in entry)
      ) {
        return undefined;
      }
      if (
        entry.customType === TMUX_SESSION_ENTRY_TYPE ||
        entry.customType === ACTIVE_DISPLAY_ENTRY_TYPE ||
        entry.customType === TMUX_DELIVERED_ENTRY_TYPE
      ) {
        this.save(entry.customType, entry.data);
      }
      return undefined;
    });
    await this.flush();
    return legacy;
  }

  save(customType: string, data: unknown, afterCommit?: () => void): void {
    const key: string = entryKey(customType, data);
    const serialized: string = JSON.stringify({ type: "custom", customType, data });
    this.#enqueue(async () => {
      await this.#harness.commit(async (tx) => {
        const doc = await tx.doc(JOURNAL);
        doc.records[key] = serialized;
      }, CONTEXT);
      afterCommit?.();
    });
  }

  assertHealthy(): void {
    if (this.#closed) {
      throw new Error("Pi Durable tmux state is closed");
    }
    if (this.#failure !== undefined) {
      throw this.#failure;
    }
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
    this.assertHealthy();
    this.#pending = this.#pending
      .then(async () => {
        if (this.#failure === undefined) {
          await change();
        }
      })
      .catch((error: unknown) => this.fail(error));
  }
}
