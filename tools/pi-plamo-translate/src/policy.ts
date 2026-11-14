// This TypeScript file is executed with Bun.
import type { ImageContent } from "@earendil-works/pi-ai";
import type {
  InputEventResult,
  InputSource,
  MarkdownMessageType,
} from "@earendil-works/pi-coding-agent";
import { isInLanguage } from "../../claude-plamo-translate/hooks/translate.ts";

export const OFF = "off";
export const SECTION_NAME = "plamo-translate";
export const STATUS_KEY = "plamo-translate";
export const WIDGET_KEY = "plamo-translate";
export const CACHE_LIMIT = 200;
export const DISPLAY_TYPES = [
  "user",
  "assistant",
  "summary",
] satisfies readonly MarkdownMessageType[];

const ENGLISH_ONLY =
  "The user writes in Japanese. Their messages are machine-translated into English before you read them. Read and write English.";

export function statusText(done: number, total: number): string {
  return `PLaMo: 翻訳中… ${String(done)}/${String(total)}`;
}

export function isCommand(text: string): boolean {
  const trimmed = text.trimStart();
  return trimmed.startsWith("/") || trimmed.startsWith("!");
}

export function shouldTranslatePrompt(
  text: string,
  source: InputSource,
  promptLanguage: string,
): boolean {
  if (source === "extension" || promptLanguage === OFF || isCommand(text)) {
    return false;
  }
  return !isInLanguage(text, promptLanguage);
}

export function replyRule(replyLanguage: string, promptLanguage: string): string | undefined {
  if (replyLanguage === OFF && promptLanguage === OFF) {
    return undefined;
  }
  if (replyLanguage === OFF) {
    return ENGLISH_ONLY;
  }
  return `The user writes in Japanese. Their messages are machine-translated into English before you read them, and your replies are machine-translated into \`${replyLanguage}\` for display only. Read and write English. Only inline code and fenced code blocks are shown exactly as written. Put anything the user must read or copy verbatim in one of them: commands, file paths, identifiers, option and flag names, literal values, error messages and quoted UI text. Do not wrap ordinary prose.`;
}

export function needsDisplayTranslation(
  text: string,
  replyLanguage: string,
  streaming: boolean,
): boolean {
  return !streaming && replyLanguage !== OFF && !isInLanguage(text, replyLanguage);
}

export function transformedInput(
  text: string,
  images: readonly ImageContent[] | undefined,
): InputEventResult {
  if (images === undefined) {
    return { action: "transform", text };
  }
  return { action: "transform", text, images: [...images] };
}

function isTextBlock(block: unknown): block is { readonly text: string; readonly type: "text" } {
  return (
    typeof block === "object" &&
    block !== null &&
    "type" in block &&
    block.type === "text" &&
    "text" in block &&
    typeof block.text === "string"
  );
}

export function textBlock(block: unknown): string | undefined {
  if (!isTextBlock(block)) {
    return undefined;
  }
  const trimmed = block.text.trim();
  return trimmed === "" ? undefined : trimmed;
}

export function assistantTexts(message: {
  readonly content?: unknown;
  readonly role: string;
}): readonly string[] {
  if (message.role !== "assistant" || !Array.isArray(message.content)) {
    return [];
  }
  return message.content.flatMap((block: unknown) => {
    const text = textBlock(block);
    return text === undefined ? [] : [text];
  });
}

export function remember<Value>(map: Map<string, Value>, key: string, value: Value): Value {
  map.delete(key);
  map.set(key, value);
  const oldest = map.size > CACHE_LIMIT ? map.keys().next().value : undefined;
  if (oldest !== undefined) {
    map.delete(oldest);
  }
  return value;
}
