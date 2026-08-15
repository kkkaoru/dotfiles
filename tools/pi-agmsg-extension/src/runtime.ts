import type {
  ActiveIdentity,
  AgmsgActionInput,
  AgmsgRuntimeDependencies,
  AgmsgService,
  IdentityCommandRequest,
  IdentityLookup,
  IdentityResolution,
  IdentityStore,
  MessageSink,
  RepeatScheduler,
  RuntimeContext,
} from "./contracts.ts";
import { AgmsgOperations, displayOutput } from "./agmsg-operations.ts";
import { AutomaticDelivery } from "./automatic-delivery.ts";
import {
  chooseIdentity,
  chooseSetupIdentity,
  identityStatus,
  resolveActiveIdentity,
  NO_TEAM_LEAVE_ERROR,
  NO_TEAM_RECONNECT_ERROR,
} from "./identity.ts";
import { leaveTeam } from "./membership.ts";
import { setupIdentity } from "./onboarding.ts";
import {
  combine,
  errorMessage,
  HELP,
  parseCommand,
  parseLimit,
  parseSend,
} from "./runtime-helpers.ts";
import { MONITOR_INTERVAL_MS } from "./scheduler.ts";

export class AgmsgRuntime {
  readonly #automaticDelivery: AutomaticDelivery;
  readonly #client: AgmsgService;
  readonly #identityStore: IdentityStore;
  readonly #messages: MessageSink;
  readonly #operations: AgmsgOperations;
  readonly #scheduler: RepeatScheduler;
  #active: ActiveIdentity | undefined;
  #deliveryOwned: boolean | undefined;
  #stopMonitor: (() => void) | undefined;

  constructor({ client, identityStore, lease, messages, scheduler }: AgmsgRuntimeDependencies) {
    this.#messages = messages;
    this.#automaticDelivery = new AutomaticDelivery({ identityStore, lease, messages });
    this.#client = client;
    this.#identityStore = identityStore;
    this.#operations = new AgmsgOperations(client, messages);
    this.#scheduler = scheduler;
  }

  async start(ctx: RuntimeContext): Promise<void> {
    this.#automaticDelivery.start();
    const identity: ActiveIdentity | undefined = await this.#resolveIdentity(ctx);
    this.#setActive(identity, ctx);
    this.#startMonitor(ctx);
  }

  async stop(ctx: RuntimeContext): Promise<void> {
    await this.#automaticDelivery.stop();
    this.#stopMonitor?.();
    this.#stopMonitor = undefined;
    this.#active = undefined;
    this.#deliveryOwned = undefined;
    ctx.ui.setStatus("agmsg", undefined);
  }

  async checkAutomatically(ctx: RuntimeContext): Promise<void> {
    await this.#automaticDelivery.check({
      activate: (identity: ActiveIdentity): void => {
        this.#active = identity;
      },
      context: ctx,
      ownership: (identity: ActiveIdentity, owned: boolean): void => {
        this.#deliveryOwned = owned;
        this.#updateStatus(identity, ctx);
      },
      receive: async (identity: ActiveIdentity): Promise<string> =>
        this.#operations.inbox({ identity, quiet: true, signal: ctx.signal }),
      resolveIdentity: async (): Promise<ActiveIdentity | undefined> =>
        this.#active ?? this.#resolveIdentity(ctx),
    });
  }

  async execute(input: AgmsgActionInput, ctx: RuntimeContext): Promise<string> {
    return this.#operations.execute({
      identity: await this.#requireIdentity(ctx),
      input,
      project: ctx.cwd,
      signal: ctx.signal,
    });
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
      return this.#operations.inbox({
        identity: request.identity,
        quiet: false,
        signal: request.context.signal,
      });
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
      return this.#operations.send({
        identity: request.identity,
        input: parseSend(request.rest),
        signal: request.context.signal,
      });
    }
    if (request.command === "leave") {
      const result: Awaited<ReturnType<typeof leaveTeam>> = await leaveTeam({
        client: this.#client,
        context: request.context,
        identity: request.identity,
        requested: request.rest,
      });
      this.#identityStore.save(result.identity);
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
    await this.stop(ctx);
    this.#automaticDelivery.start();
    const identity: ActiveIdentity = await this.#requireExistingCommandIdentity(
      ctx,
      NO_TEAM_RECONNECT_ERROR,
    );
    const owned: boolean = await this.#automaticDelivery.takeOwnership(identity);
    if (!owned) {
      throw new Error("Could not acquire the agmsg automatic-delivery lease. Retry reconnect.");
    }
    this.#deliveryOwned = true;
    this.#updateStatus(identity, ctx);
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
    const identity: ActiveIdentity | undefined = this.#active ?? (await this.#resolveIdentity(ctx));
    if (identity === undefined) {
      throw new Error("No unambiguous pi identity. Run /agmsg setup first.");
    }
    this.#active = identity;
    return identity;
  }

  async #resolveIdentity(ctx: RuntimeContext): Promise<ActiveIdentity | undefined> {
    return resolveActiveIdentity({
      client: this.#client,
      project: ctx.cwd,
      signal: ctx.signal,
      stored: this.#identityStore.load(ctx),
    });
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
      const selected: ActiveIdentity | undefined = await chooseSetupIdentity({
        client: this.#client,
        context: ctx,
      });
      if (selected !== undefined) {
        this.#setActive(selected, ctx);
        return "Selected agmsg identity for this session.";
      }
      return this.#joinNewIdentity(ctx, lookup.teams);
    }
    const available: readonly string[] =
      lookup.kind === "single" ? lookup.teams : lookup.availableTeams;
    return this.#joinNewIdentity(ctx, available);
  }

  async #joinNewIdentity(ctx: RuntimeContext, suggestedTeams: readonly string[]): Promise<string> {
    const result: Awaited<ReturnType<typeof setupIdentity>> = await setupIdentity({
      client: this.#client,
      context: ctx,
      suggestedTeams,
    });
    this.#setActive(result.identity, ctx);
    return `${result.output}\nAutomatic background and end-of-turn delivery is enabled for this pi session.`;
  }

  #startMonitor(ctx: RuntimeContext): void {
    this.#stopMonitor?.();
    this.#stopMonitor = this.#scheduler.repeat((): void => {
      this.checkAutomatically(ctx).catch((error: unknown): void => {
        ctx.ui.setStatus("agmsg", `agmsg: ${errorMessage(error)}`);
      });
    }, MONITOR_INTERVAL_MS);
  }

  async #toggleAuto(value: string, ctx: RuntimeContext): Promise<string> {
    if (value !== "on" && value !== "off") {
      throw new Error("Usage: /agmsg auto <on|off>");
    }
    await this.#automaticDelivery.setEnabled(value === "on");
    this.#deliveryOwned = value === "on" ? undefined : false;
    this.#updateStatus(this.#active, ctx);
    return `Automatic agmsg delivery (background + end-of-turn): ${value}`;
  }

  #setActive(identity: ActiveIdentity | undefined, ctx: RuntimeContext): void {
    this.#active = identity;
    this.#deliveryOwned = undefined;
    if (identity !== undefined) {
      this.#identityStore.save(identity);
    }
    this.#updateStatus(identity, ctx);
  }

  #updateStatus(identity: ActiveIdentity | undefined, ctx: RuntimeContext): void {
    const status: string | undefined = identityStatus(identity, this.#automaticDelivery.enabled);
    const displayed: string | undefined =
      status !== undefined && this.#automaticDelivery.enabled && this.#deliveryOwned === false
        ? `${status} (standby)`
        : status;
    ctx.ui.setStatus("agmsg", displayed);
  }
}
