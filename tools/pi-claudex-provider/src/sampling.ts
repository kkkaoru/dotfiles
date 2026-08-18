import { GatewayError } from "./errors.ts";
import type { GatewayRequestOptions, JsonRecord } from "./protocol.ts";

export function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function optionalSamplingParams(
  value: JsonRecord,
  id: string,
): GatewayRequestOptions["samplingParams"] {
  const raw = value["samplingParams"];
  if (raw === undefined) {
    return undefined;
  }
  if (!isRecord(raw)) {
    throw new GatewayError("Gateway option samplingParams must be an object", id);
  }
  const result: Record<string, number> = {};
  for (const [key, val] of Object.entries(raw)) {
    if (typeof val !== "number" || !Number.isFinite(val)) {
      throw new GatewayError(`Gateway option samplingParams.${key} must be a finite number`, id);
    }
    result[key] = val;
  }
  return Object.keys(result).length > 0 ? result : undefined;
}
