// This file runs with Bun.
import type { Context, Message } from "@earendil-works/pi-ai";

/**
 * Matches pi-coding-agent's COMPACTION_SUMMARY_PREFIX so a compacted context
 * forces a fresh Devin ACP session instead of appending onto pre-compact history.
 */
const COMPACTION_SUMMARY_MARKER: string =
  "The conversation history before this point was compacted into the following summary:";

const SECTION_SEPARATOR: string = "\n\n";

function contentText(content: Message["content"]): string {
  if (typeof content === "string") return content;
  return content
    .map((part) => {
      if (part.type === "text") return part.text;
      if (part.type === "thinking") return `[thinking]\n${part.thinking}`;
      if (part.type === "toolCall") {
        return `[tool call ${part.id}: ${part.name}]\n${JSON.stringify(part.arguments)}`;
      }
      return `[image: ${part.mimeType}]`;
    })
    .join("\n");
}

function formatMessage(message: Message): string {
  if (message.role === "user") return `USER:\n${contentText(message.content)}`;
  if (message.role === "assistant") return `ASSISTANT:\n${contentText(message.content)}`;
  const status: string = message.isError ? "error" : "success";
  return `TOOL RESULT (${message.toolName}, ${message.toolCallId}, ${status}):\n${contentText(message.content)}`;
}

export function buildDevinTranscript(context: Context): string {
  const sections: string[] = context.messages.map(formatMessage);
  if (context.systemPrompt) sections.unshift(`SYSTEM INSTRUCTIONS:\n${context.systemPrompt}`);
  sections.push("Continue from the transcript above. Follow the latest user request.");
  return sections.join(SECTION_SEPARATOR);
}

export function latestUserText(context: Context): string | undefined {
  const users = context.messages.filter((message) => message.role === "user");
  const last = users.at(-1);
  return last ? contentText(last.content) : undefined;
}

export function buildContinuationPrompt(context: Context): string {
  const text = latestUserText(context);
  return text === undefined ? buildDevinTranscript(context) : text;
}

export function transcriptIncludesCompaction(transcript: string): boolean {
  return transcript.includes(COMPACTION_SUMMARY_MARKER);
}
