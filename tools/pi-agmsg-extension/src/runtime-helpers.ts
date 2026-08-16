import { randomUUID } from "node:crypto";
import path from "node:path";
import type { ActiveIdentity } from "./contracts.ts";

const AGENT_ID_LENGTH = 12 satisfies number;
export const DEFAULT_HISTORY_LIMIT = 20 satisfies number;
const FIRST_SUFFIX = 2 satisfies number;
const MAX_HISTORY_LIMIT = 100 satisfies number;
const MIN_HISTORY_LIMIT = 1 satisfies number;

export const HELP = `agmsg commands:
  /agmsg                         Check inbox (runs setup on first use)
  /agmsg send <agent> <message>  Send a message
  /agmsg history [limit]         Show history
  /agmsg team                    List team members
  /agmsg leave [team]            Leave a team across all registered projects
  /agmsg reconnect               Reconnect automatic delivery after resume
  /agmsg setup                   Select or create an identity
  /agmsg auto <on|off>           Toggle automatic inbox delivery
  /agmsg whoami                  Show the active identity
  /agmsg version                 Show agmsg version`;

export interface ParsedCommand {
  readonly command: string;
  readonly rest: string;
}

export interface SendArguments {
  readonly message: string;
  readonly to: string;
}

function uniqueName(baseName: string, existingNames: readonly string[]): string {
  if (!existingNames.includes(baseName)) {
    return baseName;
  }
  const candidates: readonly string[] = Array.from(
    { length: existingNames.length + 1 },
    (_value: unknown, index: number): string => `${baseName}-${String(index + FIRST_SUFFIX)}`,
  );
  const available: string | undefined = candidates.find(
    (candidate: string): boolean => !existingNames.includes(candidate),
  );
  if (available === undefined) {
    throw new Error(`Unable to generate a unique name for ${baseName}.`);
  }
  return available;
}

export function combine(outputs: readonly string[]): string {
  return outputs.filter((output: string): boolean => output !== "").join("\n\n");
}

export function uniqueStrings(values: readonly string[]): readonly string[] {
  return [...new Set(values)];
}

export function parseCommand(args: string): ParsedCommand {
  const value: string = args.trim();
  const separator: number = value.search(/\s/u);
  return separator === -1
    ? { command: value, rest: "" }
    : { command: value.slice(0, separator), rest: value.slice(separator).trim() };
}

export function parseSend(rest: string): SendArguments {
  const parsed: ParsedCommand = parseCommand(rest);
  if (parsed.command === "" || parsed.rest === "") {
    throw new Error("Usage: /agmsg send <agent> <message>");
  }
  return { message: parsed.rest, to: parsed.command };
}

export function defaultTeamName(project: string): string {
  return path.basename(path.resolve(project)) || "project";
}

export function uniqueTeamName(baseName: string, existingNames: readonly string[]): string {
  return uniqueName(baseName, existingNames);
}

export function uniqueAgentName(existingNames: readonly string[], createId?: () => string): string {
  const id: string = (createId ?? randomUUID)().replaceAll("-", "").slice(0, AGENT_ID_LENGTH);
  return uniqueName(`pi-${id}`, existingNames);
}

export function firstTeam(identity: ActiveIdentity): string {
  const [team]: readonly (string | undefined)[] = identity.teams;
  if (team === undefined) {
    throw new Error(`Identity ${identity.agent} has no teams.`);
  }
  return team;
}

export function selectTeam(
  identity: ActiveIdentity,
  requested: string | undefined,
): ActiveIdentity {
  if (requested === undefined) {
    return identity;
  }
  if (!identity.teams.includes(requested)) {
    throw new Error(`Identity ${identity.agent} is not in team ${requested}.`);
  }
  return { agent: identity.agent, teams: [requested] };
}

export function parseLimit(value: string): number {
  if (value === "") {
    return DEFAULT_HISTORY_LIMIT;
  }
  const limit = Number(value) satisfies number;
  if (!Number.isInteger(limit) || limit < MIN_HISTORY_LIMIT || limit > MAX_HISTORY_LIMIT) {
    throw new Error("History limit must be an integer from 1 to 100.");
  }
  return limit;
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
