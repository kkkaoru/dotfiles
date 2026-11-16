// This file runs with bun.
import type { Tool, ToolReference, TranscriptContext } from "@earendil-works/pi-ai";

/**
 * pi hands providers a normalized transcript: the prompt and the tool declarations
 * travel as `system` messages instead of `Context.systemPrompt`/`Context.tools`.
 * The canonical replay helpers are not part of the extension-facing pi-ai surface,
 * so replay the two fields this provider needs.
 */

interface SystemLike {
  role: string;
  content?: string | Array<{ type?: string; text?: string }>;
  sections?: Record<string, string | null>;
  toolsAdded?: Tool[];
  toolsRemoved?: ToolReference[];
  tools?: Tool[];
}

function appendSections(sections: Record<string, string | null> | undefined, out: string[]): void {
  if (!sections || typeof sections !== "object") {
    return;
  }
  for (const key in sections) {
    const value = sections[key];
    if (typeof value === "string" && value.trim()) {
      out.push(value);
    }
  }
}

function appendContent(
  content: string | Array<{ type?: string; text?: string }> | undefined,
  out: string[],
): void {
  if (typeof content === "string") {
    if (content.trim()) {
      out.push(content);
    }
    return;
  }
  if (!Array.isArray(content)) {
    return;
  }
  for (const block of content) {
    if (typeof block.text === "string" && block.text.trim()) {
      out.push(block.text);
    }
  }
}

function addTools(target: Map<string, Tool>, list: Tool[] | undefined): void {
  if (!list) {
    return;
  }
  for (const tool of list) {
    if (typeof tool.name === "string") {
      target.set(tool.name, tool);
    }
  }
}

function removeTools(target: Map<string, Tool>, list: ToolReference[] | undefined): void {
  if (!list) {
    return;
  }
  for (const removed of list) {
    target.delete(removed.name);
  }
}

/** Prompt text after replaying every system message in order. */
export function currentSystemPrompt(messages: TranscriptContext["messages"]): string {
  const parts: string[] = [];
  for (const raw of messages) {
    if (raw.role !== "system") {
      continue;
    }
    const message = raw as unknown as SystemLike;
    appendSections(message.sections, parts);
    appendContent(message.content, parts);
  }
  return parts.join("\n\n");
}

/** Tool set after applying every declaration and removal in order. */
export function currentTools(messages: TranscriptContext["messages"]): Tool[] {
  const tools = new Map<string, Tool>();
  for (const raw of messages) {
    if (raw.role !== "system") {
      continue;
    }
    const message = raw as unknown as SystemLike;
    removeTools(tools, message.toolsRemoved);
    addTools(tools, message.toolsAdded);
    addTools(tools, message.tools);
  }
  return [...tools.values()];
}
