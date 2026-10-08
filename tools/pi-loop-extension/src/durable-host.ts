// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import type { LoopExtensionHost } from "../index.ts";
import type { LoopContext } from "./contracts.ts";
import { DurableLoopStore } from "./durable-store.ts";
import type { Scheduler } from "./scheduler.ts";
import { latestLoopState, LOOP_STATE_ENTRY_TYPE, type LoopRuntimeState } from "./state.ts";

interface DurableHost {
  readonly host: LoopExtensionHost;
  readonly scheduler: Scheduler;
}

function restoredContext(context: LoopContext, state: LoopRuntimeState | undefined): LoopContext {
  const entries =
    state === undefined ? [] : [{ type: "custom", customType: LOOP_STATE_ENTRY_TYPE, data: state }];
  const getSessionId = context.sessionManager?.getSessionId?.bind(context.sessionManager);
  return {
    ...context,
    sessionManager: {
      ...(getSessionId === undefined ? {} : { getSessionId }),
      getEntries: () => entries,
    },
  };
}

class DurableSession {
  store: DurableLoopStore | undefined;
  #release: (() => Promise<void>) | undefined;
  #ready: Promise<void> | undefined;
  #stopping: Promise<void> | undefined;
  startHandler: Parameters<LoopExtensionHost["on"]>[1] | undefined;
  stopped = false;

  assertRunning(): void {
    if (this.stopped) {
      throw new Error("Pi Durable session is stopped.");
    }
  }

  async ensureReady(context: LoopContext, event?: unknown): Promise<void> {
    this.assertRunning();
    this.#ready ??= this.#initialize(context, event);
    return this.#ready;
  }

  async #initialize(context: LoopContext, event: unknown): Promise<void> {
    try {
      const active = await this.open(context);
      await this.startHandler?.(event ?? { type: "session_start", reason: "startup" }, active);
      await this.requireStore().flush();
    } catch (error) {
      await this.close();
      throw error;
    }
  }

  async start(context: LoopContext, event: unknown): Promise<void> {
    if (this.#stopping !== undefined) {
      await this.#stopping;
      this.#stopping = undefined;
    }
    this.stopped = false;
    await this.ensureReady(context, event);
  }

  async stop(handler: () => unknown): Promise<void> {
    this.stopped = true;
    this.#stopping ??= this.#stop(handler);
    await this.#stopping;
  }

  async #stop(handler: () => unknown): Promise<void> {
    try {
      // Initialization errors are already reported by the event that awaited readiness.
      await this.#ready?.catch(() => {
        /* The originating event reports this failure. */
      });
      await handler();
    } finally {
      try {
        await this.close();
      } finally {
        this.#ready = undefined;
      }
    }
  }

  requireStore(): DurableLoopStore {
    if (this.store === undefined) {
      throw new Error("Pi Durable loop session is not ready.");
    }
    return this.store;
  }

  wrap(
    event: Parameters<LoopExtensionHost["on"]>[0],
    handler: Parameters<LoopExtensionHost["on"]>[1],
  ): Parameters<LoopExtensionHost["on"]>[1] {
    if (event === "session_start") {
      this.startHandler = handler;
    }
    return async (value, context) => {
      if (event === "session_shutdown") {
        await this.stop(async () => handler(value, context));
        return;
      }
      if (event === "session_start") {
        await this.start(context, value);
        return;
      }
      if (this.stopped) {
        return;
      }
      await this.ensureReady(context);
      if (this.stopped) {
        return;
      }
      const store = this.requireStore();
      await handler(value, context);
      if (this.store === store) {
        await store.flush();
      }
    };
  }

  async open(context: LoopContext): Promise<LoopContext> {
    await this.close();
    const file: string | undefined = context.sessionManager?.getSessionFile?.();
    const report = (error: unknown): void => {
      context.ui.notify(`Pi Durable loop stopped: ${String(error)}`, "error");
    };
    try {
      this.store = await this.#openStore(file, report);
      const state = await this.store.restore(
        latestLoopState(context.sessionManager?.getEntries() ?? []),
      );
      if (state !== undefined) {
        this.store.save(state);
        await this.store.flush();
      }
      return restoredContext(context, state);
    } catch (error) {
      await this.close();
      throw error;
    }
  }

  async #openStore(
    file: string | undefined,
    report: (error: unknown) => void,
  ): Promise<DurableLoopStore> {
    if (file === undefined) {
      return DurableLoopStore.open(new MemoryStorage(), report);
    }
    const directory = `${file}.loop-durable`;
    await mkdir(directory, { recursive: true, mode: 0o700 });
    this.#release = await lock(directory, {
      retries: 0,
      onCompromised: (error) => this.store?.fail(error),
    });
    return DurableLoopStore.open(
      await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, { fsync: true }),
      report,
    );
  }

  async close(): Promise<void> {
    const { store } = this;
    const release = this.#release;
    this.#release = undefined;
    this.store = undefined;
    try {
      await store?.close();
    } finally {
      await release?.();
    }
  }
}

export function durableHost(original: LoopExtensionHost): DurableHost {
  const session = new DurableSession();
  original.on("before_agent_start", async (_event, context) => {
    if (!session.stopped) {
      await session.ensureReady(context);
    }
  });
  return {
    scheduler: {
      now: () => Date.now(),
      clearInterval: (cancel) => cancel(),
      setInterval: (callback, interval) => {
        const { scheduler } = session.requireStore();
        if (scheduler === undefined) {
          throw new Error("Pi Durable scheduler is not ready.");
        }
        return scheduler.setInterval(callback, interval);
      },
    },
    host: {
      ...original,
      appendEntry: (customType, data) => {
        const state = latestLoopState([{ type: "custom", customType, data }]);
        if (state === undefined) {
          throw new Error("Invalid loop state write.");
        }
        const snapshot = globalThis.structuredClone(state);
        session.requireStore().save(snapshot, () => original.appendEntry?.(customType, snapshot));
      },
      flush: async () => session.requireStore().flush(),
      cancelDelivery: () => session.store?.cancelDelivery(),
      sendUserMessage: (text, options) =>
        session.requireStore().deliver(() => original.sendUserMessage(text, options)),
      registerCommand: (name, definition) =>
        original.registerCommand(name, {
          ...definition,
          handler: async (args, context) => {
            await session.ensureReady(context);
            session.assertRunning();
            await definition.handler(args, context);
          },
        }),
      on: (event, handler) => original.on(event, session.wrap(event, handler)),
    },
  };
}
