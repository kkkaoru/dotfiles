// Runs with Bun.

import type { AssistantMessage, ThinkingContent } from "@earendil-works/pi-ai";
import { isRecord } from "./protocol.ts";

const THOUGHT_RESULT_CHARACTER_LIMIT = 400 satisfies number;
const GRAPHEME_SEGMENTER = new Intl.Segmenter(undefined, { granularity: "grapheme" });
const RESPONSES_APIS = new Set<AssistantMessage["api"]>([
  "openai-responses",
  "azure-openai-responses",
  "openai-codex-responses",
]);

function thinkingBlockAt(message: AssistantMessage, index: number): ThinkingContent | undefined {
  const block = message.content[index];
  return block?.type === "thinking" ? block : undefined;
}

function responseSummary(signature: string | undefined): string {
  if (signature === undefined) {
    return "";
  }
  try {
    const parsed: unknown = JSON.parse(signature);
    if (!isRecord(parsed) || !Array.isArray(parsed["summary"])) {
      return "";
    }
    return parsed["summary"]
      .map((part) => (isRecord(part) && typeof part["text"] === "string" ? part["text"] : ""))
      .filter((part) => part.trim().length > 0)
      .join("\n\n");
  } catch {
    return "";
  }
}

function finalParagraph(content: string): string {
  return content
    .trim()
    .split(/\n\s*\n/gu)
    .map((paragraph) => paragraph.replaceAll(/\s+/gu, " ").trim())
    .filter((paragraph) => paragraph.length > 0)
    .slice(-1)
    .join("");
}

function boundedTail(content: string): string {
  const graphemes = Array.from(GRAPHEME_SEGMENTER.segment(content), ({ segment }) => segment);
  if (graphemes.length <= THOUGHT_RESULT_CHARACTER_LIMIT) {
    return content;
  }
  return `…${graphemes.slice(-(THOUGHT_RESULT_CHARACTER_LIMIT - 1)).join("")}`;
}

export function normalizeThoughtResult(
  message: AssistantMessage,
  index: number,
  terminalContent: string,
): string {
  const block = thinkingBlockAt(message, index);
  if (block?.redacted === true) {
    return "";
  }
  const summary = RESPONSES_APIS.has(message.api) ? responseSummary(block?.thinkingSignature) : "";
  return boundedTail(finalParagraph(summary.length > 0 ? summary : terminalContent));
}
