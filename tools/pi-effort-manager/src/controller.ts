// This TypeScript file is executed with Bun.
import path from "node:path";
import { RecoveryEffort, type RoundResult, type TurnEvidence } from "./recovery-effort.ts";
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
  NO_OBSERVATIONS,
  withObservation,
  profileValue,
  reasoningSummary,
  type ObservedReasoning,
} from "./display.ts";
import { fastEligible } from "./fast.ts";
import { effortProfile, selectDynamicEffort, type EffortProfile } from "./policy.ts";
import type { ProgressTextSettings } from "./progress.ts";
import {
  countCompaction,
  effectiveResetPolicy,
  latestPersistedState,
  restoredCompactionCount,
  setSessionOverride,
  STATE_ENTRY_TYPE,
  initialManagerState,
  type ManagerState,
  type SessionOverrides,
} from "./session.ts";
import {
  isRecord,
  readManagerSettings,
  readReserveTokens,
  writeManagerBoolean,
  type ManagerSettings,
} from "./settings.ts";

const modelKey = (context: ExtensionContext): string =>
  context.model === undefined ? "none" : `${context.model.provider}/${context.model.id}`;

export class EffortController {
  readonly #recovery = new RecoveryEffort();
  readonly #observed = new Map<string, Map<ModelThinkingLevel, ObservedReasoning>>();
  readonly #pi: ExtensionAPI;
  readonly #settingsPath: string;
  #activeRequestLevel: ModelThinkingLevel = "off";
  #projectSettingsPath: string | undefined = undefined;
  #reserveTokens: number;
  #settings: ManagerSettings;
  #state: ManagerState;

  constructor(pi: ExtensionAPI) {
    this.#pi = pi;
    this.#settingsPath = path.join(getAgentDir(), "settings.json");
    this.#settings = readManagerSettings(this.#settingsPath);
    this.#reserveTokens = readReserveTokens(this.#settingsPath);
    this.#state = initialManagerState(this.#settings.dynamicDefault);
  }

  get dynamicEnabled(): boolean {
    return this.#state.enabled;
  }

  updateUi(context: ExtensionContext): void {
    const level = this.#pi.getThinkingLevel();
    const dynamic = this.#state.enabled ? " · dynamic" : "";
    context.ui.setStatus("pi-effort-thinking", `think:${level}${dynamic}`);
    context.ui.setStatus(
      "pi-effort-fast",
      this.#settings.fastMode && fastEligible(context.model) ? "fast" : undefined,
    );
    context.ui.setStatus("pi-effort-dynamic", undefined);
    context.ui.setWorkingMessage(
      level === "off" ? undefined : `Working (${level} effort${dynamic})...`,
    );
  }

  setDynamic(context: ExtensionContext, enabled: boolean, persistDefault = false): void {
    if (!enabled) {
      this.compactionFailed(context);
    }
    this.#state.enabled = enabled;
    this.#state.forceBaselineTurns = enabled ? 1 : 0;
    this.#persistState();
    if (persistDefault) {
      writeManagerBoolean(this.#settingsPath, "dynamicDefault", enabled);
      this.#refreshSettings();
    }
    if (enabled) {
      this.#applyDynamic(context, true);
    } else {
      this.updateUi(context);
    }
    context.ui.notify(`Dynamic effort ${enabled ? "enabled" : "disabled"}.`, "info");
  }

  setSessionPolicy(context: ExtensionContext, key: keyof SessionOverrides, value: string): void {
    const overrides = setSessionOverride(this.#state.overrides, key, value);
    if (typeof overrides === "string") {
      context.ui.notify(overrides, "error");
      return;
    }
    this.#state.overrides = overrides;
    if (key === "compactionResetEffort" || key === "compactionResetInterval") {
      this.#state.compactionCount = 0;
      this.#state.forceBaselineTurns = 0;
    }
    this.#persistState();
    this.#applyDynamic(context, true);
    context.ui.notify(
      `Session ${key} override ${value === "default" ? "cleared" : `set to ${value}`}.`,
      "info",
    );
  }

  status(context: ExtensionContext): string {
    const profile = this.#profile(context);
    const observations = this.#observed.get(modelKey(context));
    const resetPolicy = effectiveResetPolicy(this.#state.overrides, this.#settings);
    return `${formatEffortStatus({
      compaction: profileValue(profile?.compaction),
      compactionCount: this.#state.compactionCount,
      contextTokens: contextTokens(context),
      contextWindow: contextWindow(context),
      dynamicEnabled: this.#state.enabled,
      effort: this.#pi.getThinkingLevel(),
      end: profileValue(profile?.operational.at(-1)),
      levels: profileValue(profile?.supported.join(", ")),
      reasoning: reasoningSummary(observations),
      resetEffort: resetPolicy.effort,
      resetInterval: resetPolicy.interval,
      start: profileValue(profile?.baseline),
    })} recoverySteps=${this.#recovery.snapshot().steps}`;
  }

  setFast(context: ExtensionContext, enabled: boolean): void {
    writeManagerBoolean(this.#settingsPath, "fastMode", enabled);
    this.#refreshSettings();
    this.updateUi(context);
    context.ui.notify(
      `Fast mode ${enabled ? "enabled" : "disabled"}.`,
      enabled ? "warning" : "info",
    );
  }

  fastMode(): boolean {
    return this.#settings.fastMode;
  }

  providerPayload(payload: unknown, context: ExtensionContext): unknown {
    if (!this.#settings.fastMode || !fastEligible(context.model) || !isRecord(payload)) {
      return undefined;
    }
    return payload["service_tier"] === undefined
      ? { ...payload, service_tier: "priority" }
      : undefined;
  }

  progressTextSettings(): ProgressTextSettings {
    return this.#settings;
  }

  sessionStart(context: ExtensionContext, dynamicFlag?: unknown): void {
    this.#projectSettingsPath = context.isProjectTrusted()
      ? path.join(context.cwd, CONFIG_DIR_NAME, "settings.json")
      : undefined;
    this.#refreshSettings();
    this.#reserveTokens = readReserveTokens(this.#settingsPath, this.#projectSettingsPath);
    const entries = context.sessionManager.getBranch();
    const persisted = latestPersistedState(entries);
    this.#recovery.restore(persisted.recovery, entries);
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
    this.#reapply(context, false);
  }

  modelSelected(context: ExtensionContext): boolean {
    this.#state.preCompactionLevel = undefined;
    return this.#reapply(context, false);
  }

  beforeAgentStart(context: ExtensionContext): boolean {
    const forceBaseline = this.#state.forceBaselineTurns > 0;
    this.#state.forceBaselineTurns = Math.max(0, this.#state.forceBaselineTurns - 1);
    return this.#reapply(context, forceBaseline);
  }

  turnEnded(context: ExtensionContext, evidence: TurnEvidence): boolean {
    if (this.#state.enabled && this.#recovery.observe(evidence, this.#profile(context))) {
      this.#persistState();
    }
    return this.#reapply(context, false);
  }

  claimStuckNote(context: ExtensionContext, result: RoundResult): boolean {
    return this.#state.enabled && this.#recovery.claimStuckNote(result, this.#profile(context));
  }

  #reapply(context: ExtensionContext, forceBaseline: boolean): boolean {
    const changed = this.#applyDynamic(context, forceBaseline);
    this.#activeRequestLevel = this.#pi.getThinkingLevel();
    return changed;
  }

  observeReasoning(context: ExtensionContext, reasoning: number): void {
    const key = modelKey(context);
    const byLevel = this.#observed.get(key) ?? new Map<ModelThinkingLevel, ObservedReasoning>();
    byLevel.set(
      this.#activeRequestLevel,
      withObservation(byLevel.get(this.#activeRequestLevel) ?? NO_OBSERVATIONS, reasoning),
    );
    this.#observed.set(key, byLevel);
  }

  beforeCompaction(context: ExtensionContext): void {
    if (!this.#state.enabled || this.#state.preCompactionLevel !== undefined) {
      return;
    }
    const profile = this.#profile(context);
    if (profile === undefined) {
      return;
    }
    this.#state.preCompactionLevel = this.#pi.getThinkingLevel();
    this.#setManagedLevel(context, profile.compaction);
  }

  compacted(context: ExtensionContext): boolean {
    if (!this.#state.enabled || this.#state.preCompactionLevel === undefined) {
      return false;
    }
    const outcome = countCompaction({
      level: this.#state.preCompactionLevel,
      count: this.#state.compactionCount,
      ...effectiveResetPolicy(this.#state.overrides, this.#settings),
    });
    this.#state.compactionCount = outcome.count;
    this.#state.forceBaselineTurns = Number(outcome.reset);
    this.#state.preCompactionLevel = undefined;
    this.#persistState();
    this.#applyDynamic(context, outcome.reset);
    return true;
  }

  compactionFailed(context: ExtensionContext): void {
    const previous = this.#state.preCompactionLevel;
    if (this.#state.enabled && previous !== undefined && previous !== "off") {
      this.#setManagedLevel(context, previous);
    }
    this.#state.preCompactionLevel = undefined;
  }

  #applyDynamic(context: ExtensionContext, forceBaseline = false): boolean {
    const selected =
      this.#state.enabled && this.#state.preCompactionLevel === undefined
        ? this.#selectForContext(context, forceBaseline)
        : undefined;
    if (selected === undefined) {
      this.updateUi(context);
      return false;
    }
    return this.#setManagedLevel(context, selected);
  }

  #persistState(): void {
    const resetPolicy = effectiveResetPolicy(this.#state.overrides, this.#settings);
    this.#pi.appendEntry(STATE_ENTRY_TYPE, {
      compactionCount: this.#state.compactionCount,
      enabled: this.#state.enabled,
      overrides: this.#state.overrides,
      recovery: this.#recovery.snapshot(),
      resetEffort: resetPolicy.effort,
      resetInterval: resetPolicy.interval,
    });
  }

  #profile(context: ExtensionContext): EffortProfile | undefined {
    return effortProfile(context.model, {
      compactionEffort: this.#state.overrides.compactionEffort ?? this.#settings.compactionEffort,
      endEffort: this.#state.overrides.endEffort ?? this.#settings.endEffort,
      startEffort: this.#state.overrides.startEffort ?? this.#settings.startEffort,
    });
  }

  #refreshSettings(): void {
    this.#settings = readManagerSettings(this.#settingsPath, this.#projectSettingsPath);
  }

  #selectForContext(context: ExtensionContext, forceBaseline: boolean): ThinkingLevel | undefined {
    const profile = this.#profile(context);
    if (profile === undefined || context.model === undefined) {
      return undefined;
    }
    return this.#recovery.select(
      profile,
      selectDynamicEffort({
        contextTokens: context.getContextUsage()?.tokens ?? 0,
        contextWindow: context.model.contextWindow,
        forceBaseline,
        profile,
        rampStartRatio: this.#settings.rampStartRatio,
        reserveTokens: this.#reserveTokens,
      }),
    );
  }

  #setManagedLevel(context: ExtensionContext, level: ThinkingLevel): boolean {
    const changed = this.#pi.getThinkingLevel() !== level;
    if (changed) {
      this.#pi.setThinkingLevel(level);
    }
    this.updateUi(context);
    return changed;
  }
}
