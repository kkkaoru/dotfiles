// This file runs with bun.
import { createHash } from "node:crypto";
import type { TranscriptContext } from "@earendil-works/pi-ai";

/**
 * Wire session identity.
 *
 * `agy` keeps one trajectory per conversation: `trajectory_id` stays constant,
 * `request_id` is `<trajectory_id>-<requestIndex>` where the index counts completed
 * assistant turns, `last_step_index` is the index of the last content block, and
 * later turns add `last_execution_id`. Pi owns the conversation, so the ids are
 * derived from pi's session id (plus the opening user message as a fallback for
 * headless runs), which keeps a resumed pi session on the same trajectory without
 * storing any provider-side state.
 */

export interface EnvelopeInput {
  sessionId?: string;
  messages: TranscriptContext["messages"];
  /** Number of content blocks in the outgoing request. */
  step: number;
  isClaude: boolean;
  isNonGemini: boolean;
  /** `model` field of the catalog entry, e.g. `MODEL_PLACEHOLDER_M526`. */
  modelEnum?: string;
}

export interface Envelope {
  trajectoryId: string;
  requestId: string;
  sessionId: string;
  labels: Record<string, string>;
}

const UUID_BYTE_COUNT = 16;
const UUID_VERSION_INDEX = 6;
const UUID_VARIANT_INDEX = 8;
const UUID_VERSION_5_MASK = 0x50;
const UUID_VARIANT_MASK = 0x80;
const MAX_PROMPT_SEED_LENGTH = 512;
const INT64_SIGN_BIT_MASK = 0x7fffffffffffffffn;

const sha1 = (seed: string): Buffer => createHash("sha1").update(seed).digest();

/** Deterministic UUID-shaped id (version-5 layout) from a seed. */
export function stableUuid(seed: string): string {
  const bytes = Buffer.from(sha1(seed).subarray(0, UUID_BYTE_COUNT));
  bytes[UUID_VERSION_INDEX] = (bytes[UUID_VERSION_INDEX]! & 0x0f) | UUID_VERSION_5_MASK;
  bytes[UUID_VARIANT_INDEX] = (bytes[UUID_VARIANT_INDEX]! & 0x3f) | UUID_VARIANT_MASK;
  const hex = bytes.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** Int64-shaped decimal session id, which is what the CLI sends in `request.sessionId`. */
export function stableSessionId(seed: string): string {
  const value = sha1(seed).readBigUInt64BE(0) & INT64_SIGN_BIT_MASK;
  return value.toString();
}

/** Conversation seed: pi's session id when present, otherwise the opening user turn. */
export function conversationSeed(
  sessionId: string | undefined,
  messages: TranscriptContext["messages"],
): string {
  if (sessionId?.trim()) {
    return `session:${sessionId.trim()}`;
  }
  const firstUser = messages.find((message) => message.role === "user");
  if (!firstUser) {
    return "prompt:empty";
  }
  if (typeof firstUser.content === "string") {
    return `prompt:${firstUser.content.slice(0, MAX_PROMPT_SEED_LENGTH)}`;
  }
  // Bound before serializing so large base64 image payloads are never stringified whole.
  const preview = firstUser.content.map((block) =>
    block.type === "text"
      ? { type: "text", text: block.text.slice(0, MAX_PROMPT_SEED_LENGTH) }
      : { type: block.type },
  );
  return `prompt:${JSON.stringify(preview).slice(0, MAX_PROMPT_SEED_LENGTH)}`;
}

/** Completed assistant turns; the CLI's `requestIndex`. */
export function requestIndex(messages: TranscriptContext["messages"]): number {
  const counter = { count: 0 };
  for (const message of messages) {
    if (message.role !== "assistant") {
      continue;
    }
    const stopReason = (message as { stopReason?: string }).stopReason;
    if (stopReason !== "error" && stopReason !== "aborted") {
      counter.count += 1;
    }
  }
  return counter.count;
}

export function sessionEnvelope(input: EnvelopeInput): Envelope {
  const seed = conversationSeed(input.sessionId, input.messages);
  const trajectoryId = stableUuid(`antigravity:trajectory:${seed}`);
  const index = requestIndex(input.messages);
  const step = Math.max(1, input.step);

  const labels: Record<string, string> = {
    last_step_index: String(Math.max(0, step - 1)),
    request_id: `${trajectoryId}-${index}`,
    trajectory_id: trajectoryId,
    used_claude: String(input.isClaude),
    used_claude_conservative: String(input.isClaude),
    used_non_gemini_model: String(input.isNonGemini || input.isClaude),
  };
  if (input.modelEnum) {
    labels["model_enum"] = input.modelEnum;
  }
  if (index > 0) {
    labels["last_execution_id"] = stableUuid(`antigravity:exec:${trajectoryId}:${step - 1}`);
  }

  return {
    trajectoryId,
    requestId: `${trajectoryId}-${index}`,
    sessionId: stableSessionId(`antigravity:session:${seed}`),
    labels,
  };
}
