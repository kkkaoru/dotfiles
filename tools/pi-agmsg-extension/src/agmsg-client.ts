import type { ExecOptions, ExecResult } from "@earendil-works/pi-coding-agent";
import { homedir } from "node:os";
import path from "node:path";
import type {
  AgmsgService,
  ExecHost,
  HistoryRequest,
  IdentityLookup,
  IdentityPair,
  InboxRequest,
  JoinRequest,
  LeaveRequest,
  SendRequest,
  TeamMember,
} from "./contracts.ts";

const AGENT_TYPE = "pi" satisfies string;
const DEFAULT_TIMEOUT_MS = 30_000 satisfies number;
const SCRIPTS_DIRECTORY: string = path.join(homedir(), ".agents", "skills", "agmsg", "scripts");

type Exec = (command: string, args: string[], options?: ExecOptions) => Promise<ExecResult>;
type FieldEntry = readonly [string, string];

interface ScriptRequest {
  readonly args: readonly string[];
  readonly cwd?: string | undefined;
  readonly script: string;
  readonly signal?: AbortSignal | undefined;
}

interface JsonName {
  readonly name: string;
}

interface JsonMember extends JsonName {
  readonly project?: string;
  readonly types: readonly unknown[];
}

function csv(value: string | undefined): readonly string[] {
  return value === undefined || value === "" || value === "none"
    ? []
    : value.split(",").filter((item: string): boolean => item !== "");
}

function parseField(token: string): FieldEntry | undefined {
  const separator: number = token.indexOf("=");
  return separator > 0 ? [token.slice(0, separator), token.slice(separator + 1)] : undefined;
}

function isFieldEntry(entry: FieldEntry | undefined): entry is FieldEntry {
  return entry !== undefined;
}

function fields(output: string): Readonly<Record<string, string>> {
  const entries: readonly FieldEntry[] = output
    .split(/\s+/u)
    .map((token: string): FieldEntry | undefined => parseField(token))
    .filter((entry: FieldEntry | undefined): entry is FieldEntry => isFieldEntry(entry));
  return Object.fromEntries(entries);
}

function parseJson(line: string): unknown {
  const value: unknown = JSON.parse(line);
  return value;
}

function hasStringName(value: unknown): value is JsonName {
  return (
    typeof value === "object" && value !== null && "name" in value && typeof value.name === "string"
  );
}

function isJsonMember(value: unknown): value is JsonMember {
  return hasStringName(value) && "types" in value && Array.isArray(value.types);
}

export function parseWhoami(output: string): IdentityLookup {
  const data: Readonly<Record<string, string>> = fields(output.trim());
  const availableTeams: readonly string[] = csv(data["available_teams"]);
  if (data["not_joined"] === "true") {
    return { availableTeams, kind: "not-joined" };
  }

  const agents: readonly string[] = csv(data["agents"]);
  const teams: readonly string[] = csv(data["teams"]);
  if (data["suggest"] === "true") {
    return { agents, availableTeams, kind: "suggestion", teams };
  }
  if (data["multiple"] === "true") {
    return { agents, kind: "multiple", teams };
  }
  const agent: string | undefined = data["agent"];
  if (agent === undefined) {
    throw new Error(`Unexpected whoami output: ${output}`);
  }
  return { agent, kind: "single", teams };
}

export function parseIdentityPairs(output: string): readonly IdentityPair[] {
  return output
    .split("\n")
    .filter((line: string): boolean => line !== "")
    .map((line: string): IdentityPair => {
      const [team, agent]: readonly (string | undefined)[] = line.split("\t");
      if (team === undefined || agent === undefined) {
        throw new Error(`Invalid identity: ${line}`);
      }
      return { agent, team };
    });
}

export function parseTeams(output: string): readonly string[] {
  return output
    .split("\n")
    .filter((line: string): boolean => line !== "")
    .map((line: string): string => {
      const value: unknown = parseJson(line);
      if (!hasStringName(value)) {
        throw new TypeError(`Invalid team: ${line}`);
      }
      return value.name;
    });
}

export function parseMembers(output: string): readonly TeamMember[] {
  return output
    .split("\n")
    .filter((line: string): boolean => line !== "")
    .map((line: string): TeamMember => {
      const value: unknown = parseJson(line);
      if (!isJsonMember(value)) {
        throw new TypeError(`Invalid member: ${line}`);
      }
      const types: readonly string[] = value.types.filter(
        (item: unknown): item is string => typeof item === "string",
      );
      return typeof value.project === "string"
        ? { name: value.name, project: value.project, types }
        : { name: value.name, types };
    });
}

export class AgmsgClient implements AgmsgService {
  readonly #exec: Exec;
  readonly #scriptsDirectory: string;

  constructor(exec: Exec, scriptsDirectory: string) {
    this.#exec = exec;
    this.#scriptsDirectory = scriptsDirectory;
  }

  static fromHost(host: ExecHost): AgmsgClient {
    return new AgmsgClient(host.exec.bind(host), SCRIPTS_DIRECTORY);
  }

  async whoami(project: string, signal?: AbortSignal): Promise<IdentityLookup> {
    return parseWhoami(
      await this.#run({ args: [project, AGENT_TYPE], cwd: project, script: "whoami.sh", signal }),
    );
  }

  async identities(project: string, signal?: AbortSignal): Promise<readonly IdentityPair[]> {
    const output: string = await this.#run({
      args: [project, AGENT_TYPE],
      cwd: project,
      script: "identities.sh",
      signal,
    });
    return parseIdentityPairs(output);
  }

  async inbox(request: InboxRequest): Promise<string> {
    const args: readonly string[] = request.quiet
      ? [request.team, request.agent, "--quiet"]
      : [request.team, request.agent];
    return this.#run({ args, script: "inbox.sh", signal: request.signal });
  }

  async send(request: SendRequest): Promise<string> {
    return this.#run({
      args: [request.team, request.from, request.to, request.message],
      script: "send.sh",
      signal: request.signal,
    });
  }

  async history(request: HistoryRequest): Promise<string> {
    return this.#run({
      args: [request.team, request.agent, String(request.limit)],
      script: "history.sh",
      signal: request.signal,
    });
  }

  async team(team: string, signal?: AbortSignal): Promise<string> {
    return this.#run({ args: [team], script: "team.sh", signal });
  }

  async members(team: string, signal?: AbortSignal): Promise<readonly TeamMember[]> {
    const output: string = await this.#run({
      args: ["get", "teams", team, "members"],
      script: "api.sh",
      signal,
    });
    return parseMembers(output);
  }

  async listTeams(signal?: AbortSignal): Promise<readonly string[]> {
    const output: string = await this.#run({ args: ["get", "teams"], script: "api.sh", signal });
    return parseTeams(output);
  }

  async join(request: JoinRequest): Promise<string> {
    return this.#run({
      args: [request.team, request.agent, AGENT_TYPE, request.project],
      cwd: request.project,
      script: "join.sh",
      signal: request.signal,
    });
  }

  async leave(request: LeaveRequest): Promise<string> {
    return this.#run({
      args: [request.team, request.agent],
      script: "leave.sh",
      signal: request.signal,
    });
  }

  async version(signal?: AbortSignal): Promise<string> {
    return this.#run({ args: [], script: "version.sh", signal });
  }

  async #run(request: ScriptRequest): Promise<string> {
    const options: ExecOptions = { timeout: DEFAULT_TIMEOUT_MS };
    if (request.cwd !== undefined) {
      options.cwd = request.cwd;
    }
    if (request.signal !== undefined) {
      options.signal = request.signal;
    }
    const result: ExecResult = await this.#exec(
      "bash",
      [path.join(this.#scriptsDirectory, request.script), ...request.args],
      options,
    );
    if (result.code !== 0) {
      throw new Error(this.#errorMessage(request.script, result));
    }
    return result.stdout.trimEnd();
  }

  #errorMessage(script: string, result: ExecResult): string {
    const detail: string =
      result.stderr.trim() || result.stdout.trim() || `exit ${String(result.code)}`;
    return `${script} failed: ${detail}`;
  }
}
