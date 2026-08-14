import type { ActiveIdentity, AgmsgService, IdentityLookup, RuntimeContext } from "./contracts.ts";

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

export async function lookupSingleIdentity(
  request: LookupIdentityRequest,
): Promise<ActiveIdentity | undefined> {
  try {
    const lookup: IdentityLookup = await request.client.whoami(request.project, request.signal);
    return lookup.kind === "single" ? lookup : undefined;
  } catch {
    return undefined;
  }
}
