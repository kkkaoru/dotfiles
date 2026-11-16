// This file runs with bun.
import type { TextContent, Tool, ToolReference, TranscriptContext } from "@earendil-works/pi-ai";

/**
 * pi hands providers a normalized transcript: the prompt and the tool declarations
 * travel as `system` messages instead of `Context.systemPrompt`/`Context.tools`.
 * The canonical replay helpers are not part of the extension-facing pi-ai surface,
 * so replay the two fields this provider needs.
 */

function pushNonEmpty(value: unknown, out: string[]): void {
  if (typeof value === "string" && value.trim()) {
    out.push(value);
  }
}

function appendSections(sections: Record<string, string | null> | undefined, out: string[]): void {
  if (!sections || typeof sections !== "object") {
    return;
  }
  for (const key in sections) {
    pushNonEmpty(sections[key], out);
  }
}

function appendContent(content: string | TextContent[], out: string[]): void {
  if (typeof content === "string") {
    pushNonEmpty(content, out);
    return;
  }
  if (!Array.isArray(content)) {
    return;
  }
  for (const block of content) {
    pushNonEmpty(block.text, out);
  }
}

function addTool(target: Map<string, Tool>, tool: Tool): void {
  if (typeof tool.name === "string") {
    target.set(tool.name, tool);
  }
}

function addTools(target: Map<string, Tool>, list: readonly Tool[] | undefined): void {
  if (!list) {
    return;
  }
  for (const tool of list) {
    addTool(target, tool);
  }
}

function removeTools(target: Map<string, Tool>, list: readonly ToolReference[] | undefined): void {
  if (!list) {
    return;
  }
  for (const removed of list) {
    target.delete(removed.name);
  }
}

function isToolItem(value: unknown): value is Tool {
  return (
    typeof value === "object" && value !== null && "name" in value && typeof value.name === "string"
  );
}

function legacyToolsOf(message: object): Tool[] | undefined {
  if (!("tools" in message) || !Array.isArray(message.tools)) {
    return undefined;
  }
  return message.tools.filter(isToolItem);
}

function appendSystemMessage(
  parts: string[],
  message: TranscriptContext["messages"][number],
): void {
  if (message.role !== "system") {
    return;
  }
  appendSections(message.sections, parts);
  appendContent(message.content, parts);
}

function applyToolMessage(
  tools: Map<string, Tool>,
  message: TranscriptContext["messages"][number],
): void {
  if (message.role !== "system") {
    return;
  }
  removeTools(tools, message.toolsRemoved);
  addTools(tools, message.toolsAdded);
  addTools(tools, legacyToolsOf(message));
}

export interface ReplayedTranscript {
  systemPrompt: string;
  tools: Tool[];
}

/** Replay both system prompt and active tool declarations in one pass over the transcript. */
export function replayTranscript(messages: TranscriptContext["messages"]): ReplayedTranscript {
  const parts: string[] = [];
  const tools = new Map<string, Tool>();
  for (const message of messages) {
    appendSystemMessage(parts, message);
    applyToolMessage(tools, message);
  }
  return { systemPrompt: parts.join("\n\n"), tools: [...tools.values()] };
}

/** Prompt text after replaying every system message in order. */
export function currentSystemPrompt(messages: TranscriptContext["messages"]): string {
  const parts: string[] = [];
  for (const message of messages) {
    appendSystemMessage(parts, message);
  }
  return parts.join("\n\n");
}

/** Tool set after applying every declaration and removal in order. */
export function currentTools(messages: TranscriptContext["messages"]): Tool[] {
  const tools = new Map<string, Tool>();
  for (const message of messages) {
    applyToolMessage(tools, message);
  }
  return [...tools.values()];
}
