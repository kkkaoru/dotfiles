// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { MemoryStorage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import { changedPart, fingerprint, type Fingerprint, type Part } from "./src/fingerprint.ts";
import {
  EMPTY_LEDGER,
  isMiss,
  record,
  renderLedger,
  statusText,
  type Ledger,
  type Row,
} from "./src/ledger.ts";
import { DurableLedgerStore } from "./src/store.ts";

const STATUS_KEY = "prompt-cache";

interface Session {
  readonly release: (() => Promise<void>) | undefined;
  readonly store: DurableLedgerStore;
  ledger: Ledger;
  /** The last request's prefix, so the next one can be compared with it. */
  previous: Fingerprint | undefined;
  /** Set by a request whose prefix changed: the answer to it is the recomputed prompt. */
  pending: Part;
  /** Whether a miss with an unchanged prefix was already reported in this session. */
  reported: boolean;
}

interface Usage {
  readonly cacheRead?: number;
  readonly cacheWrite?: number;
  readonly input?: number;
}

interface AssistantMessage {
  readonly model?: string;
  readonly role?: string;
  readonly usage?: Usage;
}

function reportWith(context: ExtensionContext): (error: unknown) => void {
  return (error): void => {
    context.ui.notify(`Prompt cache storage stopped: ${String(error)}`, "error");
  };
}

async function openStore(
  context: ExtensionContext,
): Promise<Omit<Session, "ledger" | "pending" | "previous" | "reported">> {
  const file: string | undefined = context.sessionManager.getSessionFile();
  const report = reportWith(context);
  if (file === undefined) {
    return {
      release: undefined,
      store: await DurableLedgerStore.open(new MemoryStorage(), report),
    };
  }
  // The sidecar stays with the session file, so a resumed session keeps its own hit rate.
  const directory = `${file}.cache-durable`;
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const release = await lock(directory, { retries: 0 });
  const storage = await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, { fsync: true });
  return { release, store: await DurableLedgerStore.open(storage, report) };
}

function rowOf(message: AssistantMessage, part: Part, at: number): Row | undefined {
  const { usage } = message;
  if (usage === undefined) {
    return undefined;
  }
  const prompt = (usage.input ?? 0) + (usage.cacheRead ?? 0) + (usage.cacheWrite ?? 0);
  return { at, cached: usage.cacheRead ?? 0, model: message.model ?? "", part, prompt };
}

export default function promptCacheExtension(pi: ExtensionAPI): void {
  let session: Session | undefined = undefined;

  const close = async (): Promise<void> => {
    const closing = session;
    session = undefined;
    try {
      await closing?.store.close();
    } finally {
      await closing?.release?.();
    }
  };

  const ensure = async (context: ExtensionContext): Promise<Session> => {
    if (session !== undefined) {
      return session;
    }
    const opened = await openStore(context);
    const stored: Ledger | undefined = await opened.store.restore();
    session = {
      ...opened,
      ledger: stored ?? EMPTY_LEDGER,
      pending: null,
      previous: undefined,
      reported: false,
    };
    return session;
  };

  pi.on("session_start", async (_event, context) => {
    await close();
    const active = await ensure(context);
    context.ui.setStatus(STATUS_KEY, statusText(active.ledger));
  });

  pi.on("before_provider_request", async (event, context) => {
    const active = await ensure(context);
    const current = fingerprint(event.payload);
    active.pending = active.previous === undefined ? null : changedPart(active.previous, current);
    active.previous = current;
  });

  pi.on("message_end", async (event, context) => {
    const message = event.message as AssistantMessage;
    if (message.role !== "assistant") {
      return;
    }
    const active = await ensure(context);
    const row = rowOf(message, active.pending, Date.now());
    if (row === undefined) {
      return;
    }
    active.pending = null;
    active.ledger = record(active.ledger, row);
    active.store.save(active.ledger);
    context.ui.setStatus(STATUS_KEY, statusText(active.ledger));
    if (!isMiss(row)) {
      return;
    }
    if (row.part !== null) {
      context.ui.notify(
        `Cache break: ${String(row.prompt)} tokens recomputed after the ${row.part} changed.`,
        "warning",
      );
    } else if (!active.reported) {
      active.reported = true;
      context.ui.notify(
        `Cache miss: ${String(row.prompt)} tokens recomputed although the prefix was unchanged; the provider dropped it.`,
        "warning",
      );
    }
  });

  pi.registerCommand("cache", {
    description: "Show this session's prompt cache hit rate and the prompts that were recomputed",
    handler: async (_args, context) => {
      const active = await ensure(context);
      context.ui.notify(renderLedger(active.ledger).join("\n"), "info");
    },
  });

  pi.on("session_shutdown", async () => {
    await close();
  });
}
