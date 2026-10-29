import type { ModelThinkingLevel } from "@earendil-works/pi-ai";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";

export interface ObservedReasoning {
  count: number;
  maximum: number;
  total: number;
}

export const NO_OBSERVATIONS: ObservedReasoning = { count: 0, maximum: 0, total: 0 };

export interface EffortStatusInput {
  compaction: string;
  compactionCount: number;
  contextTokens: number;
  contextWindow: number;
  dynamicEnabled: boolean;
  end: string;
  effort: ModelThinkingLevel;
  levels: string;
  reasoning: string;
  resetEffort: string;
  resetInterval: number;
  start: string;
}

export function profileValue(value: string | undefined): string {
  return value ?? "unavailable";
}

export function contextTokens(context: ExtensionContext): number {
  return context.getContextUsage()?.tokens ?? 0;
}

export function contextWindow(context: ExtensionContext): number {
  return context.model?.contextWindow ?? 0;
}

export function reasoningSummary(
  observations: Map<ModelThinkingLevel, ObservedReasoning> | undefined,
): string {
  if (observations === undefined) {
    return "";
  }
  return [...observations.entries()]
    .map(
      ([level, value]): string =>
        `${level}:${Math.round(value.total / value.count)}/${value.maximum}`,
    )
    .join(", ");
}

export function formatEffortStatus({
  dynamicEnabled,
  effort,
  start,
  end,
  compaction,
  resetInterval,
  resetEffort,
  contextTokens: tokens,
  contextWindow: window,
  compactionCount,
  levels,
  reasoning,
}: EffortStatusInput): string {
  return `dynamic=${dynamicEnabled ? "on" : "off"} effort=${effort} start=${start} end=${end} compact=${compaction} reset=${String(resetInterval)}@${resetEffort} context=${String(tokens)}/${String(window)} compactions=${String(compactionCount)} levels=${levels}${reasoning.length === 0 ? "" : ` reasoning(avg/max)=${reasoning}`}`;
}

export const withObservation = (
  { count, maximum, total }: ObservedReasoning,
  reasoning: number,
): ObservedReasoning => ({
  count: count + 1,
  maximum: Math.max(maximum, reasoning),
  total: total + reasoning,
});
