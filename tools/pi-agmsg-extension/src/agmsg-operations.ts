import type {
  ActiveIdentity,
  AgmsgActionInput,
  AgmsgService,
  DeliveryOptions,
  HistoryRequest,
  MessageSink,
} from "./contracts.ts";
import { combine, firstTeam, selectTeam } from "./runtime-helpers.ts";

interface ResolveTargetRequest {
  readonly identity: ActiveIdentity;
  readonly requested: string | undefined;
  readonly signal: AbortSignal | undefined;
  readonly target: string;
}

export interface RuntimeHistoryRequest {
  readonly identity: ActiveIdentity;
  readonly limit: number;
  readonly signal: AbortSignal | undefined;
  readonly team: string | undefined;
}

interface Membership {
  readonly members: readonly { readonly name: string }[];
  readonly team: string;
}

interface OutgoingMessage {
  readonly from: string;
  readonly message: string;
  readonly team: string;
  readonly to: string;
}

const PASSIVE_DELIVERY_OPTIONS: DeliveryOptions = {
  deliverAs: "steer",
  triggerTurn: false,
};

export function deliverIncoming(messages: MessageSink, output: string): void {
  messages.sendMessage(
    { content: `Incoming agmsg message:\n${output}`, customType: "agmsg-inbox", display: true },
    PASSIVE_DELIVERY_OPTIONS,
  );
}

export function displayOutgoing(messages: MessageSink, outgoing: OutgoingMessage): void {
  messages.sendMessage(
    {
      content: `Outgoing agmsg message:\nFrom: ${outgoing.from}\nTo: ${outgoing.to}\nTeam: ${outgoing.team}\n\n${outgoing.message}`,
      customType: "agmsg-sent",
      display: true,
    },
    PASSIVE_DELIVERY_OPTIONS,
  );
}

export function displayOutput(messages: MessageSink, output: string): void {
  messages.sendMessage({ content: output, customType: "agmsg-output", display: true });
}

export function renderMessageText(output: string): string {
  return output.replaceAll(String.raw`\r\n`, "\n").replaceAll(String.raw`\n`, "\n");
}

export class AgmsgOperations {
  readonly #client: AgmsgService;
  readonly #messages: MessageSink;

  constructor(client: AgmsgService, messages: MessageSink) {
    this.#client = client;
    this.#messages = messages;
  }

  async send(
    identity: ActiveIdentity,
    input: Pick<AgmsgActionInput, "message" | "team" | "to">,
    signal: AbortSignal | undefined,
  ): Promise<string> {
    if (input.to === undefined || input.message === undefined) {
      throw new Error("agmsg send requires both 'to' and 'message'");
    }
    const team: string = await this.#resolveTargetTeam({
      identity,
      requested: input.team,
      signal,
      target: input.to,
    });
    const output: string = await this.#client.send({
      from: identity.agent,
      message: input.message,
      signal,
      team,
      to: input.to,
    });
    displayOutgoing(this.#messages, {
      from: identity.agent,
      message: input.message,
      team,
      to: input.to,
    });
    return output;
  }

  async inbox(
    identity: ActiveIdentity,
    quiet: boolean,
    signal: AbortSignal | undefined,
  ): Promise<string> {
    const outputs: readonly string[] = await Promise.all(
      identity.teams.map(async (team) =>
        this.#client.inbox({ agent: identity.agent, quiet, signal, team }),
      ),
    );
    return renderMessageText(combine(outputs));
  }

  async history(request: RuntimeHistoryRequest): Promise<string> {
    const selected: ActiveIdentity = selectTeam(request.identity, request.team);
    const outputs: readonly string[] = await Promise.all(
      selected.teams.map(async (team): Promise<string> => {
        const historyRequest: HistoryRequest = {
          agent: request.identity.agent,
          limit: request.limit,
          signal: request.signal,
          team,
        };
        return this.#client.history(historyRequest);
      }),
    );
    return renderMessageText(combine(outputs));
  }

  async teams(identity: ActiveIdentity, signal: AbortSignal | undefined): Promise<string> {
    const outputs: readonly string[] = await Promise.all(
      identity.teams.map(async (team): Promise<string> => this.#client.team(team, signal)),
    );
    return combine(outputs);
  }

  async #resolveTargetTeam(request: ResolveTargetRequest): Promise<string> {
    if (request.requested !== undefined) {
      return firstTeam(selectTeam(request.identity, request.requested));
    }
    if (request.identity.teams.length === 1) {
      return firstTeam(request.identity);
    }
    const memberships: readonly Membership[] = await Promise.all(
      request.identity.teams.map(async (team): Promise<Membership> => ({
        members: await this.#client.members(team, request.signal),
        team,
      })),
    );
    const matches: readonly string[] = memberships
      .filter(({ members }): boolean =>
        members.some((member): boolean => member.name === request.target),
      )
      .map(({ team }): string => team);
    if (matches.length !== 1) {
      throw new Error(
        `Specify 'team'; target ${request.target} matched ${String(matches.length)} teams.`,
      );
    }
    return firstTeam({ agent: request.identity.agent, teams: matches });
  }
}
