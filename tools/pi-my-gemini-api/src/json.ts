// This file runs with bun.
import type { JsonObject, JsonValue } from "@earendil-works/pi-ai";

/** Parse JSON with V8's built-in parser, throwing on syntax errors. */
export function parseJson(text: string): JsonValue {
  const parsed: JsonValue = JSON.parse(text);
  return parsed;
}

/** Parse JSON, returning `undefined` on syntax errors. */
export function tryParseJson(text: string): JsonValue | undefined {
  try {
    const parsed: JsonValue = JSON.parse(text);
    return parsed;
  } catch {
    return undefined;
  }
}

/** Parse JSON, falling back to `{ raw: text }` on syntax errors. */
export function parseJsonOrRaw(text: string): JsonValue {
  const parsed = tryParseJson(text);
  return parsed === undefined ? { raw: text } : parsed;
}

/** Serialize a value to JSON. */
export const stringifyJson = (value: unknown): string => JSON.stringify(value);

/** Runtime type guard for plain JSON objects. */
export const isRecord = (value: unknown): value is JsonObject =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** Return `value` when it is a non-empty string, otherwise `undefined`. */
export const nonEmptyString = (value: unknown): string | undefined =>
  typeof value === "string" && value ? value : undefined;
