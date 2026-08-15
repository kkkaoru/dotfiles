import type {
  ActiveIdentity,
  AgmsgActionInput,
  AgmsgService,
  DeliveryOptions,
  HistoryRequest,
  MessageSink,
} from "./contracts.ts";
import {
  combine,
  DEFAULT_HISTORY_LIMIT,
  errorMessage,
  firstTeam,
  selectTeam,
} from "./runtime-helpers.ts";

interface ExecuteActionRequest {
  readonly identity: ActiveIdentity;
  readonly input: AgmsgActionInput;
  readonly project: string;
  readonly signal: AbortSignal | undefined;
}

interface InboxOperationRequest {
  readonly identity: ActiveIdentity;
  readonly quiet: boolean;
  readonly signal: AbortSignal | undefined;
}

interface SendOperationRequest {
  readonly identity: ActiveIdentity;
  readonly input: Pick<AgmsgActionInput, "message" | "team" | "to">;
  readonly signal: AbortSignal | undefined;
}

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

interface TeamInboxResult {
  readonly error: string | undefined;
  readonly output: string;
}

interface OutgoingMessage {
  readonly from: string;
  readonly message: string;
  readonly team: string;
  readonly to: string;
}

const INCOMING_DELIVERY_OPTIONS: DeliveryOptions = {
  deliverAs: "steer",
  triggerTurn: true,
};
const SENT_DELIVERY_OPTIONS: DeliveryOptions = {
  deliverAs: "steer",
  triggerTurn: false,
};

export function deliverIncoming(messages: MessageSink, output: string): void {
  messages.sendMessage(
    { content: `Incoming agmsg message:\n${output}`, customType: "agmsg-inbox", display: true },
    INCOMING_DELIVERY_OPTIONS,
  );
}

export function displayOutgoing(messages: MessageSink, outgoing: OutgoingMessage): void {
  messages.sendMessage(
    {
      content: `Outgoing agmsg message:\nFrom: ${outgoing.from}\nTo: ${outgoing.to}\nTeam: ${outgoing.team}\n\n${outgoing.message}`,
      customType: "agmsg-sent",
      display: true,
    },
    SENT_DELIVERY_OPTIONS,
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

  async execute(request: ExecuteActionRequest): Promise<string> {
    switch (request.input.action) {
      case "history": {
        return this.history({
          identity: request.identity,
          limit: request.input.limit ?? DEFAULT_HISTORY_LIMIT,
          signal: request.signal,
          team: request.input.team,
        });
      }
      case "inbox": {
        return this.inbox({
          identity: selectTeam(request.identity, request.input.team),
          quiet: false,
          signal: request.signal,
        });
      }
      case "send": {
        return this.send({
          identity: request.identity,
          input: request.input,
          signal: request.signal,
        });
      }
      case "team": {
        return this.teams(selectTeam(request.identity, request.input.team), request.signal);
      }
      case "whoami": {
        return `agent=${request.identity.agent} teams=${request.identity.teams.join(",")} type=pi project=${request.project}`;
      }
    }
  }

  async send({ identity, input, signal }: SendOperationRequest): Promise<string> {
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

  async inbox({ identity, quiet, signal }: InboxOperationRequest): Promise<string> {
    const results: readonly TeamInboxResult[] = await Promise.all(
      identity.teams.map(async (team): Promise<TeamInboxResult> => {
        try {
          const output: string = await this.#client.inbox({
            agent: identity.agent,
            quiet,
            signal,
            team,
          });
          return { error: undefined, output };
        } catch (error: unknown) {
          return { error: `Inbox failed for team ${team}: ${errorMessage(error)}`, output: "" };
        }
      }),
    );
    const outputs: readonly string[] = results.map((result): string => result.output);
    const errors: readonly string[] = results.flatMap((result): readonly string[] =>
      result.error === undefined ? [] : [result.error],
    );
    if (outputs.every((output: string): boolean => output === "") && errors.length > 0) {
      throw new Error(combine(errors));
    }
    return renderMessageText(combine([...outputs, ...errors]));
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
