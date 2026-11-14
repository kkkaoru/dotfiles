// This TypeScript file is executed with Bun.
// Project .pi/settings.json is not read; use it when one checkout needs another language.

const SETTINGS_KEY = "pi-plamo-translate";
const DEFAULT_REPLY = "ja";
const DEFAULT_PROMPT = "en";
const DEFAULT_PORT = 18555;
const MAX_PORT = 65535;

export interface PlamoSettings {
  readonly replyLanguage: string;
  readonly promptLanguage: string;
  readonly port: number;
}

const DEFAULTS: PlamoSettings = {
  replyLanguage: DEFAULT_REPLY,
  promptLanguage: DEFAULT_PROMPT,
  port: DEFAULT_PORT,
};

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function codeOf(error: unknown): unknown {
  return isRecord(error) ? error["code"] : undefined;
}

export function missingFile(error: unknown): boolean {
  return codeOf(error) === "ENOENT";
}

function parseJson(text: string): unknown {
  const value: unknown = JSON.parse(text);
  return value;
}

function language(value: unknown, fallback: string): string {
  return typeof value === "string" && value.trim() !== "" ? value : fallback;
}

function portOf(value: unknown): number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0 && value <= MAX_PORT
    ? value
    : DEFAULT_PORT;
}

function settingsFrom(value: unknown): PlamoSettings {
  if (!isRecord(value)) {
    return DEFAULTS;
  }
  const section = value[SETTINGS_KEY];
  if (!isRecord(section)) {
    return DEFAULTS;
  }
  return {
    replyLanguage: language(section["replyLanguage"], DEFAULT_REPLY),
    promptLanguage: language(section["promptLanguage"], DEFAULT_PROMPT),
    port: portOf(section["port"]),
  };
}

export function readPlamoSettings(read: () => string): PlamoSettings {
  try {
    return settingsFrom(parseJson(read()));
  } catch (error: unknown) {
    if (missingFile(error)) {
      return DEFAULTS;
    }
    throw new Error("plamo-translate: reading settings failed", { cause: error });
  }
}
