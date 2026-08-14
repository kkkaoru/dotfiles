import type { ActiveIdentity, AgmsgService, RuntimeContext } from "./contracts.ts";
import { firstTeam, selectTeam } from "./runtime-helpers.ts";

export interface LeaveResult {
  readonly identity: ActiveIdentity | undefined;
  readonly output: string;
}

export interface LeaveTeamRequest {
  readonly client: AgmsgService;
  readonly context: RuntimeContext;
  readonly identity: ActiveIdentity;
  readonly requested: string;
}

interface ChooseTeamRequest {
  readonly context: RuntimeContext;
  readonly identity: ActiveIdentity;
  readonly requested: string;
}

async function chooseTeam(request: ChooseTeamRequest): Promise<string> {
  if (request.requested !== "") {
    return firstTeam(selectTeam(request.identity, request.requested));
  }
  if (request.identity.teams.length === 1) {
    return firstTeam(request.identity);
  }
  if (!request.context.hasUI) {
    throw new Error("Multiple teams found. Use /agmsg leave <team>.");
  }
  const team: string | undefined = await request.context.ui.select(
    "Choose the agmsg team to leave",
    [...request.identity.teams],
  );
  if (team === undefined) {
    throw new Error("Team selection cancelled");
  }
  return team;
}

export async function leaveTeam(request: LeaveTeamRequest): Promise<LeaveResult> {
  const team: string = await chooseTeam({
    context: request.context,
    identity: request.identity,
    requested: request.requested.trim(),
  });
  if (!request.context.hasUI) {
    throw new Error("Leaving an agmsg team requires TUI or RPC confirmation.");
  }
  const confirmed: boolean = await request.context.ui.confirm(
    "Leave agmsg team?",
    `${request.identity.agent} will leave ${team} across all registered projects.`,
  );
  if (!confirmed) {
    return { identity: request.identity, output: "Leave cancelled." };
  }
  const output: string = await request.client.leave({
    agent: request.identity.agent,
    signal: request.context.signal,
    team,
  });
  const teams: readonly string[] = request.identity.teams.filter(
    (name: string): boolean => name !== team,
  );
  const identity: ActiveIdentity | undefined =
    teams.length === 0 ? undefined : { agent: request.identity.agent, teams };
  return { identity, output };
}
