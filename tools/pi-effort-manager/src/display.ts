import type { ModelThinkingLevel } from "@earendil-works/pi-ai";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";

export interface ObservedReasoning {
  count: number;
  maximum: number;
  total: number;
}

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

export function contextTokens(ctx: ExtensionContext): number {
  return ctx.getContextUsage()?.tokens ?? 0;
}

export function contextWindow(ctx: ExtensionContext): number {
  return ctx.model?.contextWindow ?? 0;
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

export function formatEffortStatus(input: EffortStatusInput): string {
  const observed = input.reasoning.length === 0 ? "" : ` reasoning(avg/max)=${input.reasoning}`;
  return `dynamic=${input.dynamicEnabled ? "on" : "off"} effort=${input.effort} start=${input.start} end=${input.end} compact=${input.compaction} reset=${String(input.resetInterval)}@${input.resetEffort} context=${String(input.contextTokens)}/${String(input.contextWindow)} compactions=${String(input.compactionCount)} levels=${input.levels}${observed}`;
}
