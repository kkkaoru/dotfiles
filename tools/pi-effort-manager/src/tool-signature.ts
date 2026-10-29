// This TypeScript file is executed with Bun.
import type { ToolResultMessage } from "@earendil-works/pi-ai";
import type { RoundResult } from "./recovery-effort.ts";
import { isRecord } from "./settings.ts";

interface CallInput {
  readonly name: string;
  readonly input: string;
}

// Signatures keep bounded slices, so remembered calls stay a few kilobytes whatever tools return.
const INPUT_CHARS = 512 satisfies number;
const TEXT_CHARS = 256 satisfies number;
// The model's free-text label of a call is not what the call does.
const VOLATILE_KEYS: ReadonlySet<string> = new Set(["description"]);
const SEPARATOR = "\u0000" satisfies string;
// A resumed branch re-reads only its tail; the repeat window keeps the newest results anyway.
const BRANCH_TAIL_MESSAGES = 64 satisfies number;

// Length plus head and tail, so a long write that differs mid-way is not taken for a repeat.
const boundedInput = (json: string): string =>
  `${String(json.length)}${SEPARATOR}${json.slice(0, INPUT_CHARS)}${SEPARATOR}${json.slice(-INPUT_CHARS)}`;

const dropVolatile = (key: string, value: unknown): unknown =>
  VOLATILE_KEYS.has(key) ? undefined : value;

const callEntry = (block: unknown): readonly [string, CallInput] | undefined =>
  isRecord(block) &&
  block["type"] === "toolCall" &&
  typeof block["id"] === "string" &&
  typeof block["name"] === "string"
    ? [
        block["id"],
        {
          input: boundedInput(JSON.stringify(block["arguments"], dropVolatile) ?? ""),
          name: block["name"],
        },
      ]
    : undefined;

function callInputs(message: unknown): ReadonlyMap<string, CallInput> {
  const content: unknown = isRecord(message) ? message["content"] : undefined;
  return new Map(
    (Array.isArray(content) ? content.map((block: unknown) => callEntry(block)) : []).filter(
      (entry) => entry !== undefined,
    ),
  );
}

const resultText = ({ content }: Pick<ToolResultMessage, "content">): string =>
  content.map((part) => (part.type === "text" ? part.text : "")).join("");

// A malformed call (invalid arguments, an unknown tool) needs a corrected call, not deeper reasoning.
// Stronger reasoning raises tool hallucination (Reasoning Trap, arXiv:2510.22977). Pi's own texts.
const MALFORMED_CALL = /^(Validation failed for tool|Tool "?\S+"? not found)/u;

// One result as recovery evidence, its output read once for both the signature and the error kind.
function roundResult(result: ToolResultMessage, call: CallInput | undefined): RoundResult {
  const text = resultText(result);
  const inputKey = `${result.toolName}${SEPARATOR}${call?.input ?? ""}`;
  return {
    isError: result.isError,
    malformed: result.isError && MALFORMED_CALL.test(text),
    inputKey,
    signature: [
      inputKey,
      String(result.isError),
      String(text.length),
      text.slice(0, TEXT_CHARS),
      text.slice(-TEXT_CHARS),
    ].join(SEPARATOR),
  };
}

/** The error kind of a single tool result, as the change-approach note needs it. */
export const toolResultEvidence = ({
  isError,
  content,
}: Pick<ToolResultMessage, "isError" | "content">): RoundResult => ({
  isError,
  malformed: isError && MALFORMED_CALL.test(resultText({ content })),
});

/** Pairs each tool result with what its call asked, so a repeat with the same outcome is visible. */
export function roundResults(
  message: unknown,
  results: readonly ToolResultMessage[],
): readonly RoundResult[] {
  const calls = callInputs(message);
  return results.map((result) => roundResult(result, calls.get(result.toolCallId)));
}

const isToolResult = (message: unknown): message is ToolResultMessage =>
  isRecord(message) &&
  message["role"] === "toolResult" &&
  typeof message["toolCallId"] === "string" &&
  typeof message["toolName"] === "string" &&
  typeof message["isError"] === "boolean" &&
  Array.isArray(message["content"]);

const entryMessage = (entry: unknown): unknown =>
  isRecord(entry) && entry["type"] === "message" ? entry["message"] : undefined;

/** The tool results at the tail of a session branch, so a resumed session still sees repeats. */
export function branchResults(entries: readonly unknown[]): readonly RoundResult[] {
  const messages = entries
    .map((entry: unknown) => entryMessage(entry))
    .filter((message) => message !== undefined)
    .slice(-BRANCH_TAIL_MESSAGES);
  const calls = new Map(messages.flatMap((message) => [...callInputs(message)]));
  return messages
    .filter((message) => isToolResult(message))
    .map((result) => roundResult(result, calls.get(result.toolCallId)));
}
