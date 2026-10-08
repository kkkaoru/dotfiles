// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import type { TmuxExtensionHost } from "../index.ts";
import type { CompletionDeliveryContext } from "./delivery.ts";
import { DurableTmuxState } from "./durable-state.ts";
import { TMUX_SESSION_ENTRY_TYPE, TMUX_COMPLETED_ENTRY_TYPE } from "./persistence.ts";

class DurableSession {
  state: DurableTmuxState | undefined;
  readonly pending = new Set<string>();
  #release: (() => Promise<void>) | undefined;
  #ready: Promise<void> | undefined;
  #stopping: Promise<void> | undefined;
  startHandler: Parameters<TmuxExtensionHost["on"]>[1] | undefined;
  stopped = false;

  assertRunning(): void {
    if (this.stopped) {
      throw new Error("Pi Durable session is stopped.");
    }
  }

  async ensureReady(context: CompletionDeliveryContext, event?: unknown): Promise<void> {
    this.assertRunning();
    this.#ready ??= this.#initialize(context, event);
    return this.#ready;
  }

  async #initialize(context: CompletionDeliveryContext, event: unknown): Promise<void> {
    try {
      const active = await this.open(context);
      await this.startHandler?.(event ?? { type: "session_start", reason: "startup" }, active);
      await this.requireState().flush();
    } catch (error) {
      await this.close();
      throw error;
    }
  }

  async start(context: CompletionDeliveryContext, event: unknown): Promise<void> {
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

  requireState(): DurableTmuxState {
    if (this.state === undefined) {
      throw new Error("Pi Durable tmux session is not ready");
    }
    this.state.assertHealthy();
    return this.state;
  }

  wrap(
    event: Parameters<TmuxExtensionHost["on"]>[0],
    handler: Parameters<TmuxExtensionHost["on"]>[1],
  ): Parameters<TmuxExtensionHost["on"]>[1] {
    if (event === "session_start") {
      this.startHandler = handler;
    }
    return async (value, context) => {
      if (event === "session_shutdown") {
        await this.stop(async () => handler(value, context));
        return;
      }
      if (event === "session_start") {
        if (context === undefined) {
          throw new Error("Missing Pi tmux session context");
        }
        await this.start(context, value);
        return;
      }
      if (this.stopped) {
        return;
      }
      if (context === undefined) {
        throw new Error("Missing Pi tmux session context");
      }
      await this.ensureReady(context);
      if (this.stopped) {
        return;
      }
      const state = this.requireState();
      const result: unknown = await handler(value, context);
      if (this.state === state) {
        await state.flush();
      }
      return result;
    };
  }

  async open(context: CompletionDeliveryContext): Promise<CompletionDeliveryContext> {
    await this.close();
    const { sessionManager } = context;
    if (sessionManager === undefined) {
      throw new Error("A Pi session is required for tmux ownership");
    }
    const report = (error: unknown): void =>
      context.ui.notify(`Pi Durable tmux stopped: ${String(error)}`, "error");
    try {
      const file = sessionManager.getSessionFile?.();
      this.state = await this.#openState(file, report);
      const entries = await this.state.restore(sessionManager.getEntries());
      return {
        ...context,
        sessionManager: {
          getEntries: () => entries,
          getSessionId: () => sessionManager.getSessionId(),
        },
      };
    } catch (error) {
      await this.close();
      throw error;
    }
  }

  async #openState(
    file: string | undefined,
    report: (error: unknown) => void,
  ): Promise<DurableTmuxState> {
    if (file === undefined) {
      return DurableTmuxState.open(new MemoryStorage(), report);
    }
    const directory = `${file}.tmux-durable`;
    await mkdir(directory, { recursive: true, mode: 0o700 });
    this.#release = await lock(directory, {
      retries: 0,
      onCompromised: (error) => this.state?.fail(error),
    });
    return DurableTmuxState.open(
      await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, { fsync: true }),
      report,
    );
  }

  async close(): Promise<void> {
    const { state } = this;
    const release = this.#release;
    this.#release = undefined;
    this.state = undefined;
    this.pending.clear();
    try {
      await state?.close();
    } finally {
      await release?.();
    }
  }
}

export function durableTmuxHost(original: TmuxExtensionHost): TmuxExtensionHost {
  const session = new DurableSession();
  const persist = (type: string, data: unknown): void => {
    const snapshot: unknown = globalThis.structuredClone(data);
    session.requireState().save(type, snapshot, () => original.appendEntry?.(type, snapshot));
  };
  return {
    ...original,
    appendEntry: persist,
    flush: async () => session.requireState().flush(),
    prepareLaunch: async (launch) => {
      persist(TMUX_SESSION_ENTRY_TYPE, launch);
      await session.requireState().flush();
    },
    monitor: (callback) => {
      const { monitor } = session.requireState();
      if (monitor === undefined) {
        throw new Error("Pi Durable monitor is not ready");
      }
      return monitor(callback);
    },
    pendingTasks: () => [...session.pending],
    complete: (completion, deliver) => {
      const { sessionName } = completion.launch;
      session.pending.add(sessionName);
      const state = session.requireState();
      state.save(TMUX_COMPLETED_ENTRY_TYPE, { sessionName, completion }, () => {
        if (session.state !== state) {
          return;
        }
        deliver();
        session.pending.delete(sessionName);
      });
    },
    sendUserMessage: (text, options) => {
      session.requireState().assertHealthy();
      original.sendUserMessage(text, options);
    },
    exec: async (command, args, options) => {
      const state = session.requireState();
      await state.flush();
      if (session.stopped || session.state !== state) {
        throw new Error("Pi Durable tmux session changed or stopped before execution.");
      }
      state.assertHealthy();
      return original.exec(command, args, options);
    },
    registerCommand: (name, definition) =>
      original.registerCommand?.(name, {
        ...definition,
        handler: async (args, context) => {
          await session.ensureReady(context);
          session.assertRunning();
          const state = session.requireState();
          await definition.handler(args, context);
          if (!session.stopped && session.state === state) {
            await state.flush();
          }
        },
      }),
    on: (event, handler) => original.on(event, session.wrap(event, handler)),
  };
}
