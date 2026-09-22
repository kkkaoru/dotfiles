// This TypeScript file is executed with Bun.
import { Buffer } from "node:buffer";
import { URL } from "node:url";
import { uuidv7, type Api, type Model, type ProviderHeaders } from "@earendil-works/pi-ai";
import {
  convertToLlm,
  serializeConversation,
  type ExtensionContext,
  type SessionBeforeCompactEvent,
  type CompactionResult,
} from "@earendil-works/pi-coding-agent";
import { summarizeBounded, summaryInputBudget } from "./bounded-summary.ts";

export interface GuardHost {
  readonly on: (event: "session_before_compact", handler: typeof guardedCompaction) => void;
}

export interface GuardContext {
  readonly model: ExtensionContext["model"];
  readonly modelRegistry: Pick<ExtensionContext["modelRegistry"], "complete">;
  readonly ui: Pick<ExtensionContext["ui"], "notify">;
}

export interface SessionHeaderTarget {
  readonly provider: string;
  readonly baseUrl: string;
}

const MAX_OUTPUT_TOKENS = 8192;
const OUTPUT_WINDOW_DIVISOR = 8;
const PROVIDER_MAX_RETRIES = 6;
/** Tokens Pi's own summarization prompt adds on top of the history it summarizes. */
const SUMMARIZATION_PROMPT_TOKENS = 4096;
/** Pi caps its summary output at this share of the compaction reserve. */
const SUMMARY_RESERVE_SHARE = 0.8;
/**
 * Pi's checkpoint prompt compresses a history into roughly an eighth of its tokens at its tightest,
 * so a history at 8x the output cap fills that cap and a length stop aborts the compaction. Deferring
 * at half of it leaves the summary half the cap: Pi's own records never crossed a 7.5:1 ratio, and the
 * ones on an 8192-token cap model used 86-99% of it.
 */
const SUMMARY_COMPRESSION_RATIO = 4;
const OPENCODE_HOST = "opencode.ai";
const OPENCODE_CLIENT = "pi-coding-agent";
const OPENCODE_PROVIDERS: ReadonlySet<string> = new Set(["opencode", "opencode-go"]);

function hostOf(baseUrl: string): string | undefined {
  try {
    return new URL(baseUrl).hostname;
  } catch {
    return undefined;
  }
}

/**
 * Pi summarizes a compaction in one request: the retained turns stay out of the input and the output
 * is capped by the compaction reserve, so Pi handles the compaction itself whenever that request
 * fits in the context window. Deferring to it keeps a long history in a single call instead of
 * dropping the middle of it in bounded segments.
 *
 * That single request must also be able to hold the summary Pi's prompt asks for. Pi stops it at
 * `min(0.8 * reserveTokens, maxTokens)` output tokens and treats a length stop as a failure
 * ("Summarization failed: generation hit the token cap and the summary is incomplete"), which
 * aborts the whole compaction without a fallback. A history that needs far more compression than
 * Pi's summaries achieve is therefore handled in bounded segments instead.
 */
export function piCompactionFits(
  preparation: SessionBeforeCompactEvent["preparation"],
  model: Pick<Model<Api>, "contextWindow" | "maxTokens">,
): boolean {
  const { tokensBefore, settings } = preparation;
  const reservedShare = Math.floor(SUMMARY_RESERVE_SHARE * settings.reserveTokens);
  const outputCap = model.maxTokens > 0 ? Math.min(reservedShare, model.maxTokens) : reservedShare;
  const outputTokens = Math.max(settings.reserveTokens, model.maxTokens);
  const historyTokens = tokensBefore - settings.keepRecentTokens;
  return (
    historyTokens <= SUMMARY_COMPRESSION_RATIO * outputCap &&
    historyTokens + outputTokens + SUMMARIZATION_PROMPT_TOKENS <= model.contextWindow
  );
}

/**
 * Pi's provider runner adds these headers to its own requests. Extension model calls bypass that
 * runner, and OpenCode rejects a request without the session header (MissingSessionID), so the
 * bounded compaction requests must carry them explicitly.
 */
export function providerSessionHeaders(
  model: SessionHeaderTarget,
  sessionId: string,
): ProviderHeaders {
  if (!OPENCODE_PROVIDERS.has(model.provider) && hostOf(model.baseUrl) !== OPENCODE_HOST) {
    return {};
  }
  return {
    "x-opencode-session": sessionId,
    "x-opencode-client": "pi",
    // OpenCode Go drops generic SDK/fetch user-agents; Pi's runner sets this, extension complete() does not.
    "User-Agent": OPENCODE_CLIENT,
  };
}

export async function guardedCompaction(
  event: SessionBeforeCompactEvent,
  ctx: GuardContext,
): Promise<{ compaction: CompactionResult } | { cancel: true } | undefined> {
  try {
    if (ctx.model === undefined) {
      return undefined;
    }
    const { model } = ctx;
    const { preparation } = event;
    const text: string = [
      preparation.previousSummary ?? "",
      serializeConversation(
        convertToLlm([...preparation.messagesToSummarize, ...preparation.turnPrefixMessages]),
      ),
      event.customInstructions === undefined
        ? ""
        : `Summary focus requested by user: ${event.customInstructions}`,
    ].join("\n\n");
    if (
      event.reason !== "overflow" &&
      (Buffer.byteLength(text, "utf8") < summaryInputBudget(model.contextWindow) / 2 ||
        piCompactionFits(preparation, model))
    ) {
      return undefined;
    }
    ctx.ui.notify(
      "Compacting oversized history in bounded segments. Original history is preserved.",
      "info",
    );
    const sessionId: string = uuidv7();
    const result = await summarizeBounded({
      text,
      contextWindow: model.contextWindow,
      signal: event.signal,
      onRecovery: (notice): void => ctx.ui.notify(notice, "warning"),
      complete: async (prompt) =>
        ctx.modelRegistry.complete(
          model,
          {
            messages: [
              { role: "user", content: [{ type: "text", text: prompt }], timestamp: Date.now() },
            ],
          },
          {
            signal: event.signal,
            maxTokens: Math.min(
              MAX_OUTPUT_TOKENS,
              model.maxTokens,
              Math.floor(model.contextWindow / OUTPUT_WINDOW_DIVISOR),
            ),
            reasoning: "low",
            cacheRetention: "none",
            sessionId,
            maxRetries: PROVIDER_MAX_RETRIES,
            headers: providerSessionHeaders(model, sessionId),
          },
        ),
    });
    return {
      compaction: {
        summary: result.text,
        firstKeptEntryId: preparation.firstKeptEntryId,
        tokensBefore: preparation.tokensBefore,
        usage: result.usage,
        details: {
          readFiles: [...preparation.fileOps.read],
          modifiedFiles: [
            ...new Set([...preparation.fileOps.written, ...preparation.fileOps.edited]),
          ],
        },
      },
    };
  } catch (error: unknown) {
    // Throwing from a hook would let Pi fall back to the same oversized request.
    ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
    return { cancel: true };
  }
}

export default function contextGuard(pi: GuardHost): void {
  pi.on("session_before_compact", guardedCompaction);
}
