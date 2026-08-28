// This TypeScript file is executed with Bun.
import { randomUUID } from "node:crypto";
import {
  chmodSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import type { ThinkingLevel } from "@earendil-works/pi-ai";
import {
  DEFAULT_RAMP_START_RATIO,
  DEFAULT_RESERVE_TOKENS,
  DEFAULT_RESET_COMPACTION_EFFORT,
  DEFAULT_RESET_COMPACTION_INTERVAL,
  DEFAULT_START_EFFORT,
} from "./policy.ts";

const THINKING_LEVELS = new Map<string, ThinkingLevel>([
  ["minimal", "minimal"],
  ["low", "low"],
  ["medium", "medium"],
  ["high", "high"],
  ["xhigh", "xhigh"],
  ["max", "max"],
]);

export interface ManagerSettings {
  readonly compactionEffort?: ThinkingLevel | undefined;
  readonly compactionResetEffort: ThinkingLevel;
  readonly compactionResetInterval: number;
  readonly dynamicDefault: boolean;
  readonly endEffort?: ThinkingLevel | undefined;
  readonly fastMode: boolean;
  readonly progressTextOnCompaction: boolean;
  readonly progressTextOnEffortChange: boolean;
  readonly rampStartRatio: number;
  readonly startEffort: ThinkingLevel;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function errnoCode(error: unknown): unknown {
  return isRecord(error) ? error["code"] : undefined;
}

function readObject(filePath: string): Record<string, unknown> {
  try {
    const value: unknown = JSON.parse(readFileSync(filePath, "utf8"));
    return isRecord(value) ? value : {};
  } catch (error: unknown) {
    if (errnoCode(error) === "ENOENT") {
      return {};
    }
    throw error;
  }
}

function managerObject(settings: Record<string, unknown>): Record<string, unknown> {
  const value = settings["pi-effort-manager"];
  return isRecord(value) ? { ...value } : {};
}

function thinkingLevel(value: unknown): ThinkingLevel | undefined {
  return typeof value === "string" ? THINKING_LEVELS.get(value) : undefined;
}

function positiveInteger(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : undefined;
}

function mergedManager(
  settingsPath: string,
  projectSettingsPath?: string,
): Record<string, unknown> {
  const globalManager = managerObject(readObject(settingsPath));
  const projectManager =
    projectSettingsPath === undefined ? {} : managerObject(readObject(projectSettingsPath));
  return { ...globalManager, ...projectManager };
}

export function readManagerSettings(
  settingsPath: string,
  projectSettingsPath?: string,
): ManagerSettings {
  try {
    const manager = mergedManager(settingsPath, projectSettingsPath);
    const ramp = manager["rampStartRatio"];
    return {
      compactionEffort: thinkingLevel(manager["compactionEffort"]),
      compactionResetEffort:
        thinkingLevel(manager["compactionResetEffort"]) ?? DEFAULT_RESET_COMPACTION_EFFORT,
      compactionResetInterval:
        positiveInteger(manager["compactionResetInterval"]) ?? DEFAULT_RESET_COMPACTION_INTERVAL,
      dynamicDefault: manager["dynamicDefault"] === true,
      endEffort: thinkingLevel(manager["endEffort"]),
      fastMode: manager["fastMode"] === true,
      progressTextOnCompaction: manager["progressTextOnCompaction"] === true,
      progressTextOnEffortChange: manager["progressTextOnEffortChange"] === true,
      rampStartRatio:
        typeof ramp === "number" && Number.isFinite(ramp) ? ramp : DEFAULT_RAMP_START_RATIO,
      startEffort: thinkingLevel(manager["startEffort"]) ?? DEFAULT_START_EFFORT,
    };
  } catch {
    return {
      compactionResetEffort: DEFAULT_RESET_COMPACTION_EFFORT,
      compactionResetInterval: DEFAULT_RESET_COMPACTION_INTERVAL,
      dynamicDefault: false,
      fastMode: false,
      progressTextOnCompaction: false,
      progressTextOnEffortChange: false,
      rampStartRatio: DEFAULT_RAMP_START_RATIO,
      startEffort: DEFAULT_START_EFFORT,
    };
  }
}

export function readReserveTokens(settingsPath: string, projectSettingsPath?: string): number {
  const globalCompaction = readObject(settingsPath)["compaction"];
  const projectCompaction =
    projectSettingsPath === undefined ? undefined : readObject(projectSettingsPath)["compaction"];
  const globalReserve = isRecord(globalCompaction) ? globalCompaction["reserveTokens"] : undefined;
  const projectReserve = isRecord(projectCompaction)
    ? projectCompaction["reserveTokens"]
    : undefined;
  const reserve = projectReserve ?? globalReserve;
  return typeof reserve === "number" && Number.isSafeInteger(reserve) && reserve >= 0
    ? reserve
    : DEFAULT_RESERVE_TOKENS;
}

export function resolveSettingsTarget(
  settingsPath: string,
  inspect: typeof lstatSync = lstatSync,
): { readonly mode?: number; readonly path: string } {
  try {
    const statistics = inspect(settingsPath);
    const { mode } = statistics;
    return { mode, path: statistics.isSymbolicLink() ? realpathSync(settingsPath) : settingsPath };
  } catch (error: unknown) {
    if (errnoCode(error) === "ENOENT") {
      return { path: settingsPath };
    }
    throw error;
  }
}

export function writeManagerBoolean(
  settingsPath: string,
  key: "dynamicDefault" | "fastMode",
  enabled: boolean,
  write: typeof writeFileSync = writeFileSync,
  remove: typeof rmSync = rmSync,
): void {
  const settings = readObject(settingsPath);
  const manager = managerObject(settings);
  manager[key] = enabled;
  settings["pi-effort-manager"] = manager;
  const target = resolveSettingsTarget(settingsPath);
  mkdirSync(path.dirname(target.path), { recursive: true });
  const temporary = path.join(
    path.dirname(target.path),
    `.settings.json.tmp.${String(process.pid)}.${randomUUID()}`,
  );
  try {
    write(temporary, `${JSON.stringify(settings, null, 2)}\n`, "utf8");
    if (target.mode !== undefined) {
      chmodSync(temporary, target.mode);
    }
    renameSync(temporary, target.path);
  } catch (error: unknown) {
    remove(temporary, { force: true });
    throw error;
  }
}
