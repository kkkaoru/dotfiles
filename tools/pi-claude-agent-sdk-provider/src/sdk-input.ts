// This file runs with Bun.
import type { SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import {
  renderSystemMessageUpdate,
  withoutInitialSystemMessage,
  type ImageContent,
  type Message,
  type SystemMessage,
  type TextContent,
  type UserMessage,
} from "@earendil-works/pi-ai";

type SdkContentBlock = Exclude<SDKUserMessage["message"]["content"], string>[number];
type ConversationMessage = Exclude<Message, { role: "system" }>;
type SeedMessage = SystemMessage | UserMessage;
type ImageMediaType = "image/gif" | "image/jpeg" | "image/png" | "image/webp";

const SECTION_SEPARATOR = "\n\n";
const IMAGE_MEDIA_TYPES: ReadonlySet<string> = new Set([
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/webp",
]);
const TRANSCRIPT_FOOTER =
  "Continue from the transcript above. Earlier tool calls already ran; follow the latest request.";

function isImageMediaType(value: string): value is ImageMediaType {
  return IMAGE_MEDIA_TYPES.has(value);
}

function imageBlock(part: ImageContent): SdkContentBlock {
  const mediaType = part.mimeType;
  return isImageMediaType(mediaType)
    ? { type: "image", source: { type: "base64", media_type: mediaType, data: part.data } }
    : { type: "text", text: `[unsupported image type: ${mediaType}]` };
}

function toSdkBlock(part: ImageContent | TextContent): SdkContentBlock {
  return part.type === "text" ? { type: "text", text: part.text } : imageBlock(part);
}

function userParts(message: UserMessage): (ImageContent | TextContent)[] {
  return typeof message.content === "string"
    ? [{ type: "text", text: message.content }]
    : message.content;
}

function sdkUserMessage(content: SdkContentBlock[]): SDKUserMessage {
  return {
    type: "user",
    message: { role: "user", content },
    parent_tool_use_id: null,
  };
}

function updateBlock(update: SystemMessage): SdkContentBlock {
  return { type: "text", text: renderSystemMessageUpdate(update) };
}

/**
 * Pi may queue several user messages (steering) before the next model turn. Later system
 * messages have no slot in a live SDK session, so their rendered update travels with the input.
 */
export function userMessagesToSdk(
  messages: readonly UserMessage[],
  updates: readonly SystemMessage[],
): SDKUserMessage {
  return sdkUserMessage([
    ...updates.map((update) => updateBlock(update)),
    ...messages.flatMap((message) => userParts(message).map((part) => toSdkBlock(part))),
  ]);
}

/** Text for a tool result that must also carry system updates issued with it. */
export function systemUpdateText(update: SystemMessage): string {
  return renderSystemMessageUpdate(update);
}

function partText(part: ConversationMessage["content"][number] | string): string {
  if (typeof part === "string") {
    return part;
  }
  switch (part.type) {
    case "image": {
      return `[image: ${part.mimeType}]`;
    }
    case "text": {
      return part.text;
    }
    case "toolCall": {
      return `[tool call ${part.id}: ${part.name}]\n${JSON.stringify(part.arguments)}`;
    }
    case "thinking": {
      // Thinking stays private to the turn that produced it.
      return "";
    }
  }
}

function messageText(message: ConversationMessage): string {
  const parts = typeof message.content === "string" ? [message.content] : message.content;
  return parts
    .map((part) => partText(part))
    .filter((text) => text.length > 0)
    .join("\n");
}

function formatMessage(message: Message): string {
  if (message.role === "system") {
    return `SYSTEM UPDATE:\n${renderSystemMessageUpdate(message)}`;
  }
  if (message.role === "toolResult") {
    const status = message.isError ? "error" : "success";
    return `TOOL RESULT (${message.toolName}, ${message.toolCallId}, ${status}):\n${messageText(message)}`;
  }
  return `${message.role.toUpperCase()}:\n${messageText(message)}`;
}

function isSeed(message: Message): message is SeedMessage {
  return message.role === "system" || message.role === "user";
}

/**
 * Starts a fresh SDK session from pi's transcript after its leading system prompt. Input with
 * no earlier replies is sent natively (keeping images); longer histories are replayed as text
 * because the SDK accepts only user-role input.
 */
export function initialSdkMessage(messages: readonly Message[]): SDKUserMessage {
  const rest = withoutInitialSystemMessage([...messages]);
  const seeds = rest.flatMap((message) => (isSeed(message) ? [message] : []));
  const users = seeds.flatMap((message) => (message.role === "user" ? [message] : []));
  if (seeds.length === rest.length && users.length > 0) {
    const updates = seeds.flatMap((message) => (message.role === "system" ? [message] : []));
    return userMessagesToSdk(users, updates);
  }
  const transcript = [...rest.map((message) => formatMessage(message)), TRANSCRIPT_FOOTER].join(
    SECTION_SEPARATOR,
  );
  return sdkUserMessage([{ type: "text", text: transcript }]);
}
