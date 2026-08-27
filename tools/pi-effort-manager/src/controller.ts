import path from "node:path";
import type { ModelThinkingLevel, ThinkingLevel } from "@earendil-works/pi-ai";
import {
  CONFIG_DIR_NAME,
  getAgentDir,
  type ExtensionAPI,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import {
  contextTokens,
  contextWindow,
  formatEffortStatus,
  profileValue,
  reasoningSummary,
  type ObservedReasoning,
} from "./display.ts";
import {
  effortAtLeast,
  effortProfile,
  selectDynamicEffort,
  shouldResetAfterCompaction,
  type EffortBoundaries,
  type EffortProfile,
} from "./policy.ts";
import {
  effectiveResetPolicy,
  latestPersistedState,
  restoredCompactionCount,
  setSessionOverride,
  STATE_ENTRY_TYPE,
  type SessionOverrides,
} from "./session.ts";
import {
  readManagerSettings,
  readReserveTokens,
  writeManagerBoolean,
  type ManagerSettings,
} from "./settings.ts";
const FAST_PROVIDERS = new Set(["openai", "openai-codex", "azure-openai-responses"]);

interface ManagerState {
  compactionCount: number;
  enabled: boolean;
  forceBaselineTurns: number;
  overrides: SessionOverrides;
  preCompactionLevel: ModelThinkingLevel | undefined;
}
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function modelKey(ctx: ExtensionContext): string {
  return ctx.model === undefined ? "none" : `${ctx.model.provider}/${ctx.model.id}`;
}

function fastEligible(model: ExtensionContext["model"]): boolean {
  return model !== undefined && FAST_PROVIDERS.has(model.provider) && model.id.startsWith("gpt-5");
}

export class EffortController {
  readonly #observed = new Map<string, Map<ModelThinkingLevel, ObservedReasoning>>();
  readonly #pi: ExtensionAPI;
  readonly #settingsPath: string;
  #activeRequestLevel: ModelThinkingLevel;
  #projectSettingsPath: string | undefined;
  #reserveTokens: number;
  #settings: ManagerSettings;
  #state: ManagerState;

  constructor(pi: ExtensionAPI) {
    this.#pi = pi;
    this.#settingsPath = path.join(getAgentDir(), "settings.json");
    this.#settings = readManagerSettings(this.#settingsPath);
    this.#projectSettingsPath = undefined;
    this.#reserveTokens = readReserveTokens(this.#settingsPath);
    this.#state = {
      compactionCount: 0,
      enabled: this.#settings.dynamicDefault,
      forceBaselineTurns: 0,
      overrides: {},
      preCompactionLevel: undefined,
    };
    this.#activeRequestLevel = "off";
  }

  get dynamicEnabled(): boolean {
    return this.#state.enabled;
  }

  updateUi(ctx: ExtensionContext): void {
    const level = this.#pi.getThinkingLevel();
    const dynamic = this.#state.enabled ? " · dynamic" : "";
    ctx.ui.setStatus("pi-effort-thinking", `think:${level}${dynamic}`);
    ctx.ui.setStatus(
      "pi-effort-fast",
      this.#settings.fastMode && fastEligible(ctx.model) ? "fast" : undefined,
    );
    ctx.ui.setStatus("pi-effort-dynamic", undefined);
    ctx.ui.setWorkingMessage(
      level === "off" ? undefined : `Working (${level} effort${dynamic})...`,
    );
  }

  setDynamic(ctx: ExtensionContext, enabled: boolean, persistDefault = false): void {
    this.#state.enabled = enabled;
    this.#state.forceBaselineTurns = enabled ? 1 : 0;
    this.#persistState();
    if (persistDefault) {
      writeManagerBoolean(this.#settingsPath, "dynamicDefault", enabled);
      this.#refreshSettings();
    }
    if (enabled) {
      this.#applyDynamic(ctx, true);
    } else {
      this.updateUi(ctx);
    }
    ctx.ui.notify(`Dynamic effort ${enabled ? "enabled" : "disabled"}.`, "info");
  }

  setSessionPolicy(
    ctx: ExtensionContext,
    key:
      | "startEffort"
      | "endEffort"
      | "compactionEffort"
      | "compactionResetEffort"
      | "compactionResetInterval",
    value: string,
  ): void {
    const overrides = setSessionOverride(this.#state.overrides, key, value);
    if (typeof overrides === "string") {
      ctx.ui.notify(overrides, "error");
      return;
    }
    this.#state.overrides = overrides;
    if (key === "compactionResetEffort" || key === "compactionResetInterval") {
      this.#state.compactionCount = 0;
      this.#state.forceBaselineTurns = 0;
    }
    this.#persistState();
    this.#applyDynamic(ctx, true);
    ctx.ui.notify(
      `Session ${key} override ${value === "default" ? "cleared" : `set to ${value}`}.`,
      "info",
    );
  }

  status(ctx: ExtensionContext): string {
    const profile = this.#profile(ctx);
    const observations = this.#observed.get(modelKey(ctx));
    const resetPolicy = effectiveResetPolicy(this.#state.overrides, this.#settings);
    return formatEffortStatus({
      compaction: profileValue(profile?.compaction),
      compactionCount: this.#state.compactionCount,
      contextTokens: contextTokens(ctx),
      contextWindow: contextWindow(ctx),
      dynamicEnabled: this.#state.enabled,
      effort: this.#pi.getThinkingLevel(),
      end: profileValue(profile?.operational.at(-1)),
      levels: profileValue(profile?.supported.join(", ")),
      reasoning: reasoningSummary(observations),
      resetEffort: resetPolicy.effort,
      resetInterval: resetPolicy.interval,
      start: profileValue(profile?.baseline),
    });
  }

  setFast(ctx: ExtensionContext, enabled: boolean): void {
    writeManagerBoolean(this.#settingsPath, "fastMode", enabled);
    this.#refreshSettings();
    this.updateUi(ctx);
    ctx.ui.notify(`Fast mode ${enabled ? "enabled" : "disabled"}.`, enabled ? "warning" : "info");
  }

  fastMode(): boolean {
    return this.#settings.fastMode;
  }

  providerPayload(payload: unknown, ctx: ExtensionContext): unknown {
    if (!this.#settings.fastMode || !fastEligible(ctx.model) || !isRecord(payload)) {
      return undefined;
    }
    return payload["service_tier"] === undefined
      ? { ...payload, service_tier: "priority" }
      : undefined;
  }

  sessionStart(ctx: ExtensionContext, dynamicFlag: unknown): void {
    this.#projectSettingsPath = ctx.isProjectTrusted()
      ? path.join(ctx.cwd, CONFIG_DIR_NAME, "settings.json")
      : undefined;
    this.#refreshSettings();
    this.#reserveTokens = readReserveTokens(this.#settingsPath, this.#projectSettingsPath);
    const entries = ctx.sessionManager.getBranch();
    const persisted = latestPersistedState(entries);
    const overrides = persisted.overrides ?? {};
    const resetPolicy = effectiveResetPolicy(overrides, this.#settings);
    this.#state = {
      compactionCount: restoredCompactionCount(persisted, resetPolicy),
      enabled: persisted.enabled ?? this.#settings.dynamicDefault,
      forceBaselineTurns: 0,
      overrides,
      preCompactionLevel: undefined,
    };
    if (dynamicFlag === "on" || dynamicFlag === "off") {
      this.#state.enabled = dynamicFlag === "on";
    }
    this.#applyDynamic(ctx, this.#state.enabled);
    this.#activeRequestLevel = this.#pi.getThinkingLevel();
  }

  modelSelected(ctx: ExtensionContext): void {
    this.#applyDynamic(ctx);
  }

  beforeAgentStart(ctx: ExtensionContext): void {
    const forceBaseline = this.#state.forceBaselineTurns > 0;
    this.#applyDynamic(ctx, forceBaseline);
    this.#state.forceBaselineTurns = Math.max(0, this.#state.forceBaselineTurns - 1);
    this.#activeRequestLevel = this.#pi.getThinkingLevel();
  }

  turnEnded(ctx: ExtensionContext): void {
    this.#applyDynamic(ctx);
    this.#activeRequestLevel = this.#pi.getThinkingLevel();
  }

  observeReasoning(ctx: ExtensionContext, reasoning: number): void {
    const key = modelKey(ctx);
    const byLevel = this.#observed.get(key) ?? new Map<ModelThinkingLevel, ObservedReasoning>();
    const current = byLevel.get(this.#activeRequestLevel) ?? { count: 0, maximum: 0, total: 0 };
    byLevel.set(this.#activeRequestLevel, {
      count: current.count + 1,
      maximum: Math.max(current.maximum, reasoning),
      total: current.total + reasoning,
    });
    this.#observed.set(key, byLevel);
  }

  beforeCompaction(ctx: ExtensionContext): void {
    if (!this.#state.enabled) {
      return;
    }
    const profile = this.#profile(ctx);
    if (profile === undefined) {
      return;
    }
    this.#state.preCompactionLevel = this.#pi.getThinkingLevel();
    this.#setManagedLevel(ctx, profile.compaction);
  }

  compacted(ctx: ExtensionContext): void {
    if (!this.#state.enabled) {
      return;
    }
    const resetPolicy = effectiveResetPolicy(this.#state.overrides, this.#settings);
    const qualifies = effortAtLeast(this.#state.preCompactionLevel, resetPolicy.effort);
    this.#state.compactionCount += qualifies ? 1 : 0;
    const reset =
      qualifies && shouldResetAfterCompaction(this.#state.compactionCount, resetPolicy.interval);
    this.#state.forceBaselineTurns = reset ? 1 : 0;
    this.#state.preCompactionLevel = undefined;
    this.#persistState();
    this.#applyDynamic(ctx, reset);
  }

  compactionFailed(ctx: ExtensionContext): void {
    const previous = this.#state.preCompactionLevel;
    if (this.#state.enabled && previous !== undefined && previous !== "off") {
      this.#setManagedLevel(ctx, previous);
    }
    this.#state.preCompactionLevel = undefined;
  }

  #applyDynamic(ctx: ExtensionContext, forceBaseline = false): void {
    if (!this.#state.enabled) {
      this.updateUi(ctx);
      return;
    }
    const selected = this.#selectForContext(ctx, forceBaseline);
    if (selected === undefined) {
      this.updateUi(ctx);
    } else {
      this.#setManagedLevel(ctx, selected);
    }
  }

  #persistState(): void {
    const resetPolicy = effectiveResetPolicy(this.#state.overrides, this.#settings);
    this.#pi.appendEntry(STATE_ENTRY_TYPE, {
      compactionCount: this.#state.compactionCount,
      enabled: this.#state.enabled,
      overrides: this.#state.overrides,
      resetEffort: resetPolicy.effort,
      resetInterval: resetPolicy.interval,
    });
  }

  #profile(ctx: ExtensionContext): EffortProfile | undefined {
    const boundaries: EffortBoundaries = {
      compactionEffort: this.#state.overrides.compactionEffort ?? this.#settings.compactionEffort,
      endEffort: this.#state.overrides.endEffort ?? this.#settings.endEffort,
      startEffort: this.#state.overrides.startEffort ?? this.#settings.startEffort,
    };
    return effortProfile(ctx.model, boundaries);
  }

  #refreshSettings(): void {
    this.#settings = readManagerSettings(this.#settingsPath, this.#projectSettingsPath);
  }

  #selectForContext(ctx: ExtensionContext, forceBaseline: boolean): ThinkingLevel | undefined {
    const profile = this.#profile(ctx);
    if (profile === undefined || ctx.model === undefined) {
      return undefined;
    }
    return selectDynamicEffort({
      contextTokens: ctx.getContextUsage()?.tokens ?? 0,
      contextWindow: ctx.model.contextWindow,
      forceBaseline,
      profile,
      rampStartRatio: this.#settings.rampStartRatio,
      reserveTokens: this.#reserveTokens,
    });
  }

  #setManagedLevel(ctx: ExtensionContext, level: ThinkingLevel): void {
    if (this.#pi.getThinkingLevel() !== level) {
      this.#pi.setThinkingLevel(level);
    }
    this.updateUi(ctx);
  }
}
