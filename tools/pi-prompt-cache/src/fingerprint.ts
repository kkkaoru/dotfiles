// This TypeScript file is executed with Bun.
import { createHash } from "node:crypto";

export interface Fingerprint {
  /** One hash per message, so a normal turn can be recognised as a prefix of the next request. */
  readonly messages: readonly string[];
  readonly model: string;
  readonly system: string;
  readonly tools: string;
}

export type Part = "messages" | "model" | "system" | "tools" | null;

interface Body {
  readonly messages?: unknown;
  readonly model?: unknown;
  readonly system?: unknown;
  readonly tools?: unknown;
}

function hash(value: unknown): string {
  return createHash("sha1")
    .update(JSON.stringify(value) ?? "")
    .digest("hex")
    .slice(0, 12);
}

function systemOf(body: Body): unknown {
  const { messages, system } = body;
  if (system !== undefined) {
    return system;
  }
  const first = Array.isArray(messages)
    ? (messages[0] as { role?: string } | undefined)
    : undefined;
  const { role } = first ?? {};
  return role === "system" ? first : undefined;
}

/** The parts a provider caches as one prompt prefix. */
export function fingerprint(payload: unknown): Fingerprint {
  const body = (typeof payload === "object" && payload !== null ? payload : {}) as Body;
  const { messages, tools } = body;
  const list = Array.isArray(messages) ? messages : [];
  return {
    messages: list.map((message: unknown) => hash(message)),
    model: hash(body.model),
    system: hash(systemOf(body)),
    tools: hash(tools),
  };
}

/** Which part broke the prefix, earliest first; null when the previous request is a prefix of this one. */
export function changedPart(previous: Fingerprint, current: Fingerprint): Part {
  // Every model has its own cache, so a switch is cold even when the prompt is identical.
  if (previous.model !== current.model) {
    return "model";
  }
  if (previous.system !== current.system) {
    return "system";
  }
  if (previous.tools !== current.tools) {
    return "tools";
  }
  const before = previous.messages;
  const after = current.messages;
  if (after.length < before.length) {
    return "messages";
  }
  const unchanged = before.every((messageHash, index) => after[index] === messageHash);
  return unchanged ? null : "messages";
}
