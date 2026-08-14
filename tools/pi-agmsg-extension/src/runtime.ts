import type {
  ActiveIdentity,
  AgmsgActionInput,
  AgmsgService,
  IdentityCommandRequest,
  IdentityLookup,
  IdentityResolution,
  MessageSink,
  RepeatScheduler,
  RuntimeContext,
} from "./contracts.ts";
import { AgmsgOperations, deliverIncoming, displayOutput } from "./agmsg-operations.ts";
import {
  chooseIdentity,
  lookupSingleIdentity,
  NO_TEAM_LEAVE_ERROR,
  NO_TEAM_RECONNECT_ERROR,
} from "./identity.ts";
import { leaveTeam } from "./membership.ts";
import { promptIdentity } from "./onboarding.ts";
import {
  combine,
  DEFAULT_HISTORY_LIMIT,
  errorMessage,
  firstTeam,
  HELP,
  parseCommand,
  parseLimit,
  parseSend,
  selectTeam,
} from "./runtime-helpers.ts";
import { MONITOR_INTERVAL_MS } from "./scheduler.ts";

export class AgmsgRuntime {
  readonly #client: AgmsgService;
  readonly #messages: MessageSink;
  readonly #operations: AgmsgOperations;
  readonly #scheduler: RepeatScheduler;
  #active: ActiveIdentity | undefined;
  #autoDelivery = true satisfies boolean;
  #checking = false satisfies boolean;
  #stopMonitor: (() => void) | undefined;
  #stopped = false satisfies boolean;

  constructor(messages: MessageSink, client: AgmsgService, scheduler: RepeatScheduler) {
    this.#messages = messages;
    this.#client = client;
    this.#operations = new AgmsgOperations(client);
    this.#scheduler = scheduler;
  }

  async start(ctx: RuntimeContext): Promise<void> {
    this.#stopped = false;
    const identity: ActiveIdentity | undefined = await lookupSingleIdentity({
      client: this.#client,
      project: ctx.cwd,
      signal: ctx.signal,
    });
    this.#setActive(identity, ctx);
    this.#startMonitor(ctx);
  }

  stop(ctx: RuntimeContext): void {
    this.#stopped = true;
    this.#stopMonitor?.();
    this.#stopMonitor = undefined;
    this.#active = undefined;
    ctx.ui.setStatus("agmsg", undefined);
  }

  async checkAutomatically(ctx: RuntimeContext): Promise<void> {
    if (!this.#autoDelivery || this.#checking || this.#stopped) {
      return;
    }
    this.#checking = true;
    try {
      const identity: ActiveIdentity | undefined =
        this.#active ??
        (await lookupSingleIdentity({
          client: this.#client,
          project: ctx.cwd,
          signal: ctx.signal,
        }));
      if (identity === undefined) {
        return;
      }
      this.#active = identity;
      const output: string = await this.#operations.inbox(identity, true, ctx.signal);
      if (output === "") {
        return;
      }
      deliverIncoming(this.#messages, output);
    } catch (error: unknown) {
      ctx.ui.setStatus("agmsg", `agmsg: ${errorMessage(error)}`);
    } finally {
      this.#checking = false;
    }
  }

  async execute(input: AgmsgActionInput, ctx: RuntimeContext): Promise<string> {
    const identity: ActiveIdentity = await this.#requireIdentity(ctx);
    switch (input.action) {
      case "history": {
        return this.#operations.history({
          identity,
          limit: input.limit ?? DEFAULT_HISTORY_LIMIT,
          signal: ctx.signal,
          team: input.team,
        });
      }
      case "inbox": {
        return this.#operations.inbox(selectTeam(identity, input.team), false, ctx.signal);
      }
      case "send": {
        return this.#operations.send(identity, input, ctx.signal);
      }
      case "team": {
        return this.#operations.teams(selectTeam(identity, input.team), ctx.signal);
      }
      case "whoami": {
        return `agent=${identity.agent} teams=${identity.teams.join(",")} type=pi project=${ctx.cwd}`;
      }
    }
  }

  async command(args: string, ctx: RuntimeContext): Promise<void> {
    const parsed: ReturnType<typeof parseCommand> = parseCommand(args);
    try {
      const output: string = await this.#runCommand(parsed.command, parsed.rest, ctx);
      if (output !== "") {
        displayOutput(this.#messages, output);
      }
    } catch (error: unknown) {
      if (parsed.command === "leave") {
        throw error;
      }
      ctx.ui.notify(errorMessage(error), "error");
    }
  }

  async #runCommand(command: string, rest: string, ctx: RuntimeContext): Promise<string> {
    if (command === "help") {
      return HELP;
    }
    if (command === "setup") {
      return this.#setup(ctx);
    }
    if (command === "auto") {
      return this.#toggleAuto(rest, ctx);
    }
    if (command === "version") {
      return this.#client.version(ctx.signal);
    }
    if (command === "reconnect") {
      return this.#reconnect(ctx);
    }
    if (command === "leave") {
      const identity: ActiveIdentity = await this.#requireExistingCommandIdentity(
        ctx,
        NO_TEAM_LEAVE_ERROR,
      );
      return this.#runIdentityCommand({ command, context: ctx, identity, rest });
    }

    const resolution: IdentityResolution = await this.#ensureCommandIdentity(ctx);
    if (resolution.notice !== undefined && command === "") {
      return resolution.notice;
    }
    const output: string = await this.#runIdentityCommand({
      command,
      context: ctx,
      identity: resolution.identity,
      rest,
    });
    return combine([resolution.notice ?? "", output]);
  }

  async #runIdentityCommand(request: IdentityCommandRequest): Promise<string> {
    if (request.command === "") {
      return this.#operations.inbox(request.identity, false, request.context.signal);
    }
    if (request.command === "team") {
      return this.#operations.teams(request.identity, request.context.signal);
    }
    if (request.command === "whoami") {
      return `agent=${request.identity.agent} teams=${request.identity.teams.join(",")} type=pi`;
    }
    if (request.command === "history") {
      return this.#operations.history({
        identity: request.identity,
        limit: parseLimit(request.rest),
        signal: request.context.signal,
        team: undefined,
      });
    }
    if (request.command === "send") {
      return this.#operations.send(
        request.identity,
        parseSend(request.rest),
        request.context.signal,
      );
    }
    if (request.command === "leave") {
      const result: Awaited<ReturnType<typeof leaveTeam>> = await leaveTeam({
        client: this.#client,
        context: request.context,
        identity: request.identity,
        requested: request.rest,
      });
      this.#setActive(result.identity, request.context);
      return result.output;
    }
    throw new Error(`Unknown agmsg command: ${request.command}\n\n${HELP}`);
  }

  async #ensureCommandIdentity(ctx: RuntimeContext): Promise<IdentityResolution> {
    if (this.#active !== undefined) {
      return { identity: this.#active };
    }
    const lookup: IdentityLookup = await this.#client.whoami(ctx.cwd, ctx.signal);
    if (lookup.kind === "single") {
      this.#setActive(lookup, ctx);
      return { identity: lookup };
    }
    if (lookup.kind === "multiple") {
      return { identity: await this.#chooseIdentity(ctx) };
    }
    const notice: string = await this.#setup(ctx, lookup);
    if (this.#active === undefined) {
      throw new Error("agmsg setup was cancelled");
    }
    return { identity: this.#active, notice };
  }

  async #reconnect(ctx: RuntimeContext): Promise<string> {
    this.stop(ctx);
    this.#stopped = false;
    const identity: ActiveIdentity = await this.#requireExistingCommandIdentity(
      ctx,
      NO_TEAM_RECONNECT_ERROR,
    );
    this.#startMonitor(ctx);
    await this.checkAutomatically(ctx);
    return `Reconnected agmsg as ${identity.agent} in ${identity.teams.join(",")}.`;
  }

  async #requireExistingCommandIdentity(
    ctx: RuntimeContext,
    missingMessage: string,
  ): Promise<ActiveIdentity> {
    if (this.#active !== undefined) {
      return this.#active;
    }
    const lookup: IdentityLookup = await this.#client.whoami(ctx.cwd, ctx.signal);
    if (lookup.kind === "single") {
      this.#setActive(lookup, ctx);
      return lookup;
    }
    if (lookup.kind === "multiple") {
      return this.#chooseIdentity(ctx);
    }
    throw new Error(missingMessage);
  }

  async #requireIdentity(ctx: RuntimeContext): Promise<ActiveIdentity> {
    const identity: ActiveIdentity | undefined =
      this.#active ??
      (await lookupSingleIdentity({ client: this.#client, project: ctx.cwd, signal: ctx.signal }));
    if (identity === undefined) {
      throw new Error("No unambiguous pi identity. Run /agmsg setup first.");
    }
    this.#active = identity;
    return identity;
  }

  async #chooseIdentity(ctx: RuntimeContext): Promise<ActiveIdentity> {
    const identity: ActiveIdentity = await chooseIdentity({ client: this.#client, context: ctx });
    this.#setActive(identity, ctx);
    return identity;
  }

  async #setup(ctx: RuntimeContext, known?: IdentityLookup): Promise<string> {
    if (!ctx.hasUI) {
      throw new Error("agmsg setup requires TUI or RPC mode");
    }
    const lookup: IdentityLookup = known ?? (await this.#client.whoami(ctx.cwd, ctx.signal));
    if (lookup.kind === "multiple") {
      await this.#chooseIdentity(ctx);
      return "Selected agmsg identity for this session.";
    }
    const available: readonly string[] =
      lookup.kind === "single" ? lookup.teams : lookup.availableTeams;
    const identity: ActiveIdentity = await promptIdentity({
      client: this.#client,
      context: ctx,
      suggestedTeams: available,
    });
    const team: string = firstTeam(identity);
    const output: string = await this.#client.join({
      agent: identity.agent,
      project: ctx.cwd,
      signal: ctx.signal,
      team,
    });
    this.#setActive(identity, ctx);
    return `${output}\nAutomatic background and end-of-turn delivery is enabled for this pi session.`;
  }

  #startMonitor(ctx: RuntimeContext): void {
    this.#stopMonitor?.();
    this.#stopMonitor = this.#scheduler.repeat((): void => {
      this.checkAutomatically(ctx).catch((error: unknown): void => {
        ctx.ui.setStatus("agmsg", `agmsg: ${errorMessage(error)}`);
      });
    }, MONITOR_INTERVAL_MS);
  }

  #toggleAuto(value: string, ctx: RuntimeContext): string {
    if (value !== "on" && value !== "off") {
      throw new Error("Usage: /agmsg auto <on|off>");
    }
    this.#autoDelivery = value === "on";
    this.#setActive(this.#active, ctx);
    return `Automatic agmsg delivery (background + end-of-turn): ${value}`;
  }

  #setActive(identity: ActiveIdentity | undefined, ctx: RuntimeContext): void {
    this.#active = identity;
    const suffix: string = this.#autoDelivery ? "" : " (manual)";
    ctx.ui.setStatus(
      "agmsg",
      identity === undefined
        ? undefined
        : `agmsg: ${identity.agent} (${identity.teams.join(",")})${suffix}`,
    );
  }
}
