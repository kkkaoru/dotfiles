import type {
  CompactionResult,
  ExtensionAPI,
  SessionBeforeCompactEvent,
} from "@earendil-works/pi-coding-agent";
import { convertToLlm, serializeConversation } from "@earendil-works/pi-coding-agent";
import type { Api, Message, Model, Usage } from "@earendil-works/pi-ai";
import { uuidv7 } from "@earendil-works/pi-ai";

/**
 * Cursor reports oversized requests as usage-guideline blocks instead of
 * overflow errors, so compaction summaries built from the whole conversation
 * frequently fail moderation on the Cursor side. Summarize with an off-Cursor
 * fallback chain instead; providers are ordered by availability robustness
 * because per-provider rate limits can make any single one unusable.
 */
const SUMMARY_MODELS: readonly SummaryModelRef[] = [
  { provider: "ollama-cloud", modelId: "kimi-k3" },
  { provider: "github-copilot", modelId: "gemini-3.7-flash" },
  { provider: "commandcode", modelId: "gemini-3.7-flash" },
];

interface SummaryModelRef {
  readonly provider: string;
  readonly modelId: string;
}

const CURSOR_PROVIDER = "cursor";
const SUMMARY_MAX_TOKENS = 8192;

// Mirrors pi's built-in SUMMARIZATION_SYSTEM_PROMPT, which is not exported
// through the package's public API.
const SUMMARIZATION_SYSTEM_PROMPT =
  "You are a context summarization assistant. Your task is to read a conversation " +
  "between a user and an AI assistant, then produce a structured summary following " +
  "the exact format specified.\n\n" +
  "Do NOT continue the conversation. Do NOT respond to any questions in the conversation. " +
  "ONLY output the structured summary.";

const SUMMARIZATION_PROMPT = `The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.`;

const UPDATE_SUMMARIZATION_PROMPT = `The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages.`;

export interface ResolvedSummaryModel {
  readonly model: Model<Api>;
  readonly provider: string;
  readonly modelId: string;
}

interface ModelRegistryLike {
  find(provider: string, modelId: string): Model<Api> | undefined;
  hasConfiguredAuth(model: Model<Api>): boolean;
  complete(
    model: Model<Api>,
    context: unknown,
    options?: unknown,
  ): Promise<{ content: unknown[]; usage?: Usage }>;
}

interface ExtensionContextLike {
  modelRegistry: ModelRegistryLike;
}

export function textOfContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((part): part is { type: "text"; text: string } => {
      if (typeof part !== "object" || part === null) return false;
      if (!("type" in part)) return false;
      return part.type === "text" && "text" in part && typeof part.text === "string";
    })
    .map((part) => part.text)
    .join("\n");
}

export function resolveSummaryModel(registry: ModelRegistryLike): ResolvedSummaryModel | undefined {
  for (const { provider, modelId } of SUMMARY_MODELS) {
    const model = registry.find(provider, modelId);
    if (model && registry.hasConfiguredAuth(model)) {
      return { model, provider, modelId };
    }
  }
  return undefined;
}

export function buildSummaryPrompt(
  messagesText: string,
  previousSummary: string | undefined,
  customInstructions: string | undefined,
): string {
  let basePrompt = previousSummary ? UPDATE_SUMMARIZATION_PROMPT : SUMMARIZATION_PROMPT;
  if (customInstructions) {
    basePrompt = `${basePrompt}\n\nAdditional focus: ${customInstructions}`;
  }
  let promptText = `<conversation>\n${messagesText}\n</conversation>\n\n`;
  if (previousSummary) {
    promptText += `<previous-summary>\n${previousSummary}\n</previous-summary>\n\n`;
  }
  return `${promptText}${basePrompt}`;
}

export function summarizeWithFallbackChain(
  conversationText: string,
  previousSummary: string | undefined,
  customInstructions: string | undefined,
  registry: ModelRegistryLike,
  signal: AbortSignal | undefined,
): Promise<{ summary: string; provider: string; modelId: string } | undefined> {
  const resolved = resolveSummaryModel(registry);
  if (!resolved) return Promise.resolve(undefined);
  const resolvedIndex = SUMMARY_MODELS.findIndex(
    ({ provider, modelId }) => provider === resolved.provider && modelId === resolved.modelId,
  );

  const attempt = async (
    index: number,
  ): Promise<{ summary: string; provider: string; modelId: string } | undefined> => {
    const ref = SUMMARY_MODELS[index];
    if (!ref) return undefined;
    const model =
      index === resolvedIndex ? resolved.model : registry.find(ref.provider, ref.modelId);
    if (!model || !registry.hasConfiguredAuth(model)) {
      return attempt(index + 1);
    }
    try {
      const prompt = buildSummaryPrompt(conversationText, previousSummary, customInstructions);
      const response = await registry.complete(
        model,
        {
          systemPrompt: SUMMARIZATION_SYSTEM_PROMPT,
          messages: [
            {
              role: "user",
              content: [{ type: "text", text: prompt }],
              timestamp: Date.now(),
            },
          ],
        },
        {
          maxTokens: SUMMARY_MAX_TOKENS,
          signal,
          cacheRetention: "none",
          sessionId: uuidv7(),
        },
      );
      if (signal?.aborted) return undefined;
      const summary = textOfContent(response.content).trim();
      if (!summary) {
        return attempt(index + 1);
      }
      return { summary, provider: ref.provider, modelId: ref.modelId };
    } catch {
      if (signal?.aborted) return undefined;
      return attempt(index + 1);
    }
  };

  return attempt(resolvedIndex < 0 ? 0 : resolvedIndex);
}

export function isCursorModelActive(
  event: SessionBeforeCompactEvent,
  activeModel: Model<Api> | undefined,
): boolean {
  if (activeModel) return activeModel.provider === CURSOR_PROVIDER;
  // No active model (e.g. scripted compaction): fall back to inspecting the
  // summarized messages' originating providers.
  const sawOtherProvider = event.preparation.messagesToSummarize.some((message) => {
    if (message.role !== "assistant") return false;
    const provider = (message as unknown as { provider?: string }).provider;
    return typeof provider === "string" && provider !== CURSOR_PROVIDER;
  });
  return !sawOtherProvider;
}

export function registerCursorCompaction(pi: Pick<ExtensionAPI, "on">): void {
  pi.on("session_before_compact", async (event, ctx) => {
    if (!isCursorModelActive(event, ctx.model)) return;

    const { preparation, customInstructions, signal } = event;
    const allMessages = [...preparation.messagesToSummarize, ...preparation.turnPrefixMessages];
    const conversationText = serializeConversation(convertToLlm(allMessages) as Message[]);

    const result = await summarizeWithFallbackChain(
      conversationText,
      preparation.previousSummary,
      customInstructions,
      (ctx as ExtensionContextLike).modelRegistry,
      signal,
    );
    if (!result) return undefined;

    const compaction: CompactionResult = {
      summary: result.summary,
      firstKeptEntryId: preparation.firstKeptEntryId,
      tokensBefore: preparation.tokensBefore,
    };
    return { compaction };
  });
}
