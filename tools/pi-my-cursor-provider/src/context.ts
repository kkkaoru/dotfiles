import type { SDKImage, SDKJsonValue, SDKUserMessage } from "@cursor/sdk";
import type { Context, ImageContent, Message, ToolResultMessage } from "@earendil-works/pi-ai";

const SECTION_SEPARATOR = "\n\n";

function imageToSdk(image: ImageContent): SDKImage {
  return { data: image.data, mimeType: image.mimeType };
}

function contentText(content: Message["content"]): string {
  if (typeof content === "string") return content;
  return content
    .map((part) => {
      if (part.type === "text") return part.text;
      if (part.type === "thinking") return `[thinking]\n${part.thinking}`;
      if (part.type === "toolCall") {
        return `[tool call ${part.id}: ${part.name}]\n${JSON.stringify(part.arguments)}`;
      }
      return "[image attached]";
    })
    .join("\n");
}

function formatMessage(message: Message): string {
  if (message.role === "user") return `USER:\n${contentText(message.content)}`;
  if (message.role === "assistant") return `ASSISTANT:\n${contentText(message.content)}`;
  const status = message.isError ? "error" : "success";
  return `TOOL RESULT (${message.toolName}, ${message.toolCallId}, ${status}):\n${contentText(message.content)}`;
}

export function buildCursorMessage(context: Context): SDKUserMessage {
  const sections = context.messages.map(formatMessage);
  if (context.systemPrompt) sections.unshift(`SYSTEM INSTRUCTIONS:\n${context.systemPrompt}`);
  sections.push("Continue from the transcript above. Follow the latest user request.");

  const latestUser = context.messages.findLast((message) => message.role === "user");
  const images =
    latestUser && typeof latestUser.content !== "string"
      ? latestUser.content
          .filter((part): part is ImageContent => part.type === "image")
          .map(imageToSdk)
      : [];
  return {
    text: sections.join(SECTION_SEPARATOR),
    ...(images.length > 0 ? { images } : {}),
  };
}

export function findToolResults(context: Context): ToolResultMessage[] {
  return context.messages.filter(
    (message): message is ToolResultMessage => message.role === "toolResult",
  );
}

export function toSdkJsonValue(value: unknown): SDKJsonValue | undefined {
  if (value === null || typeof value === "string" || typeof value === "boolean") return value;
  if (typeof value === "number") return Number.isFinite(value) ? value : undefined;
  if (Array.isArray(value)) {
    const converted = value.map(toSdkJsonValue);
    return converted.every((item) => item !== undefined) ? converted : undefined;
  }
  if (typeof value !== "object") return undefined;

  const entries = Object.entries(value).map(
    ([key, item]) => [key, toSdkJsonValue(item)] as [string, SDKJsonValue | undefined],
  );
  if (entries.some(([, item]) => item === undefined)) return undefined;
  return Object.fromEntries(
    entries.filter((entry): entry is [string, SDKJsonValue] => entry[1] !== undefined),
  );
}

export function toolResultToSdk(message: ToolResultMessage): SDKJsonValue {
  return {
    content: message.content.map((part) =>
      part.type === "text"
        ? { type: "text", text: part.text }
        : { type: "image", data: part.data, mimeType: part.mimeType },
    ),
    isError: message.isError,
  };
}
