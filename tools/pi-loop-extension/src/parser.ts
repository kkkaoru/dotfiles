const LEADING_INTERVAL = /^(?<amount>\d+)(?<unit>[smhd])(?:\s+(?<prompt>[\s\S]*))?$/iu;
const TRAILING_INTERVAL =
  /^(?<prompt>[\s\S]*?)\s+every\s+(?<amount>\d+)\s*(?<unit>s(?:ec(?:ond)?s?)?|m(?:in(?:ute)?s?)?|h(?:(?:ou)?r)?s?|d(?:ay)?s?)$/iu;
const SECOND_MS = 1000;
const MINUTE_MS = 60 * SECOND_MS;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
const MIN_INTERVAL_MS = MINUTE_MS;
const MAX_INTERVAL_MS = 30 * DAY_MS;

export type LoopCommand =
  | { readonly kind: "clear" }
  | { readonly kind: "list" }
  | { readonly intervalMs?: number; readonly kind: "start"; readonly prompt: string };

interface IntervalMatch {
  readonly amount: string;
  readonly prompt?: string;
  readonly unit: string;
}

type IntervalUnit = "d" | "h" | "m" | "s";

function normalizeUnit(unit: string): IntervalUnit {
  const first: string | undefined = unit.toLowerCase().at(0);
  if (first === "d") {
    return "d";
  }
  if (first === "h") {
    return "h";
  }
  if (first === "m") {
    return "m";
  }
  return "s";
}

function unitMultiplier(unit: IntervalUnit): number {
  if (unit === "d") {
    return DAY_MS;
  }
  if (unit === "h") {
    return HOUR_MS;
  }
  if (unit === "m") {
    return MINUTE_MS;
  }
  return SECOND_MS;
}

function intervalMilliseconds({ amount, unit }: IntervalMatch): number {
  const milliseconds: number = Number(amount) * unitMultiplier(normalizeUnit(unit));
  if (!Number.isSafeInteger(milliseconds) || milliseconds <= 0) {
    throw new Error("Loop interval must be a positive safe integer");
  }
  if (milliseconds > MAX_INTERVAL_MS) {
    throw new Error("Loop interval cannot exceed 30 days");
  }
  return Math.max(milliseconds, MIN_INTERVAL_MS);
}

export function matchGroups(match: RegExpMatchArray): IntervalMatch {
  const groups: Record<string, string | undefined> | undefined = match.groups;
  if (groups?.["amount"] === undefined || groups["unit"] === undefined) {
    throw new Error("Invalid loop interval");
  }
  const prompt: string | undefined = groups["prompt"]?.trim();
  return prompt === undefined
    ? { amount: groups["amount"], unit: groups["unit"] }
    : { amount: groups["amount"], prompt, unit: groups["unit"] };
}

export function parseLoopCommand(args: string): LoopCommand {
  const input: string = args.trim();
  if (input === "list") {
    return { kind: "list" };
  }
  if (input === "clear") {
    return { kind: "clear" };
  }

  const leading: RegExpExecArray | null = LEADING_INTERVAL.exec(input);
  if (leading !== null) {
    const parsed: IntervalMatch = matchGroups(leading);
    return {
      intervalMs: intervalMilliseconds(parsed),
      kind: "start",
      prompt: parsed.prompt ?? "",
    };
  }

  const trailing: RegExpExecArray | null = TRAILING_INTERVAL.exec(input);
  if (trailing !== null) {
    const parsed: IntervalMatch = matchGroups(trailing);
    return {
      intervalMs: intervalMilliseconds(parsed),
      kind: "start",
      prompt: parsed.prompt ?? "",
    };
  }
  return { kind: "start", prompt: input };
}

export function formatInterval(intervalMs: number): string {
  const units: readonly (readonly [number, string])[] = [
    [DAY_MS, "d"],
    [HOUR_MS, "h"],
    [MINUTE_MS, "m"],
  ];
  const match: readonly [number, string] | undefined = units.find(
    ([milliseconds]: readonly [number, string]): boolean => intervalMs % milliseconds === 0,
  );
  return match === undefined
    ? `${String(Math.ceil(intervalMs / MINUTE_MS))}m`
    : `${String(intervalMs / match[0])}${match[1]}`;
}
