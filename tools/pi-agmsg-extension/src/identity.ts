import type { ActiveIdentity, AgmsgService, IdentityLookup, RuntimeContext } from "./contracts.ts";

const CREATE_IDENTITY = "Create a new identity…" satisfies string;

export const NO_TEAM_LEAVE_ERROR =
  "Cannot leave an agmsg team because this pi agent is not registered in any team." satisfies string;
export const NO_TEAM_RECONNECT_ERROR =
  "Cannot reconnect agmsg because this pi agent is not registered in any team." satisfies string;

export interface IdentityRequest {
  readonly client: AgmsgService;
  readonly context: RuntimeContext;
}

export interface LookupIdentityRequest {
  readonly client: AgmsgService;
  readonly project: string;
  readonly signal: AbortSignal | undefined;
  readonly stored: ActiveIdentity | undefined;
}

export async function chooseIdentity(request: IdentityRequest): Promise<ActiveIdentity> {
  if (!request.context.hasUI) {
    throw new Error("Multiple pi identities found; run /agmsg setup in TUI mode.");
  }
  const pairs: Awaited<ReturnType<AgmsgService["identities"]>> = await request.client.identities(
    request.context.cwd,
    request.context.signal,
  );
  const options: string[] = [...new Set(pairs.map((pair): string => pair.agent))];
  const agent: string | undefined = await request.context.ui.select(
    "Choose the pi identity for this session",
    options,
  );
  if (agent === undefined) {
    throw new Error("Identity selection cancelled");
  }
  return {
    agent,
    teams: pairs.filter((pair): boolean => pair.agent === agent).map((pair): string => pair.team),
  };
}

export async function chooseSetupIdentity(
  request: IdentityRequest,
): Promise<ActiveIdentity | undefined> {
  const pairs: Awaited<ReturnType<AgmsgService["identities"]>> = await request.client.identities(
    request.context.cwd,
    request.context.signal,
  );
  const agents: string[] = [...new Set(pairs.map((pair): string => pair.agent))];
  const selected: string | undefined = await request.context.ui.select(
    "Choose an existing pi identity or create a new one",
    [...agents, CREATE_IDENTITY],
  );
  if (selected === undefined) {
    throw new Error("Identity selection cancelled");
  }
  if (selected === CREATE_IDENTITY) {
    return undefined;
  }
  return {
    agent: selected,
    teams: pairs
      .filter((pair): boolean => pair.agent === selected)
      .map((pair): string => pair.team),
  };
}

export function identityStatus(
  identity: ActiveIdentity | undefined,
  automaticDelivery: boolean,
): string | undefined {
  if (identity === undefined) {
    return undefined;
  }
  const suffix: string = automaticDelivery ? "" : " (manual)";
  return `agmsg: ${identity.agent} (${identity.teams.join(",")})${suffix}`;
}

export async function resolveActiveIdentity(
  request: LookupIdentityRequest,
): Promise<ActiveIdentity | undefined> {
  try {
    const lookup: IdentityLookup = await request.client.whoami(request.project, request.signal);
    if (lookup.kind === "single") {
      return lookup;
    }
    if (lookup.kind !== "multiple" || request.stored === undefined) {
      return undefined;
    }
    const pairs: Awaited<ReturnType<AgmsgService["identities"]>> = await request.client.identities(
      request.project,
      request.signal,
    );
    const teams: string[] = pairs
      .filter((pair): boolean => pair.agent === request.stored?.agent)
      .map((pair): string => pair.team);
    return teams.length === 0 ? undefined : { agent: request.stored.agent, teams };
  } catch {
    return request.stored;
  }
}
