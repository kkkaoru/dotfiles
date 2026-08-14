import type { ActiveIdentity, AgmsgService, RuntimeContext } from "./contracts.ts";
import { defaultTeamName, uniqueAgentName, uniqueTeamName } from "./runtime-helpers.ts";

const CREATE_TEAM = "Create a new team…" satisfies string;

export interface PromptIdentityRequest {
  readonly client: AgmsgService;
  readonly context: RuntimeContext;
  readonly suggestedTeams: readonly string[];
}

async function chooseTeam(request: PromptIdentityRequest): Promise<string> {
  const discovered: readonly string[] = await request.client.listTeams(request.context.signal);
  const existing: readonly string[] = [
    ...new Set([...request.suggestedTeams, ...discovered]),
  ].toSorted();
  const selected: string | undefined = await request.context.ui.select(
    "Choose an existing agmsg team or create one",
    [...existing, CREATE_TEAM],
  );
  if (selected === undefined) {
    throw new Error("Team selection cancelled");
  }
  if (selected !== CREATE_TEAM) {
    return selected;
  }
  const defaultTeam: string = uniqueTeamName(defaultTeamName(request.context.cwd), existing);
  const entered: string | undefined = await request.context.ui.editor(
    "New agmsg team name",
    defaultTeam,
  );
  if (entered === undefined) {
    throw new Error("Team creation cancelled");
  }
  const team: string = entered.trim() || defaultTeam;
  if (existing.includes(team)) {
    throw new Error(`Team ${team} already exists; select it from the team list.`);
  }
  return team;
}

export async function promptIdentity(request: PromptIdentityRequest): Promise<ActiveIdentity> {
  const team: string = await chooseTeam(request);
  const members: Awaited<ReturnType<AgmsgService["members"]>> = await request.client.members(
    team,
    request.context.signal,
  );
  const defaultAgent: string = uniqueAgentName(members.map((member): string => member.name));
  const agentInput: string | undefined = await request.context.ui.editor(
    "pi agent name",
    defaultAgent,
  );
  if (agentInput === undefined) {
    throw new Error("Agent selection cancelled");
  }
  return { agent: agentInput.trim() || defaultAgent, teams: [team] };
}
