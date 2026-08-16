import { describe, expect, it, vi } from "vitest";
import type {
  ActiveIdentity,
  AgmsgService,
  DeliveryLease,
  HistoryRequest,
  IdentityLookup,
  InboxRequest,
  IdentityStore,
  JoinRequest,
  LeaveRequest,
  MessageSink,
  RepeatScheduler,
  RuntimeContext,
  SendRequest,
} from "./contracts.ts";
import { AgmsgRuntime } from "./runtime.ts";

interface SchedulerState {
  canceled: boolean;
  intervalMs: number | undefined;
  task: (() => void) | undefined;
}

interface DeliveryHarness {
  readonly client: AgmsgService;
  readonly clientMock: {
    readonly inbox: ReturnType<typeof vi.fn<AgmsgService["inbox"]>>;
    readonly join: ReturnType<typeof vi.fn<AgmsgService["join"]>>;
    readonly send: ReturnType<typeof vi.fn<AgmsgService["send"]>>;
    readonly whoami: ReturnType<typeof vi.fn<AgmsgService["whoami"]>>;
  };
  readonly context: RuntimeContext;
  readonly identityStore: IdentityStore & {
    readonly save: ReturnType<typeof vi.fn<IdentityStore["save"]>>;
    readonly savePending: ReturnType<typeof vi.fn<IdentityStore["savePending"]>>;
  };
  readonly lease: DeliveryLease & {
    readonly claim: ReturnType<typeof vi.fn<DeliveryLease["claim"]>>;
    readonly release: ReturnType<typeof vi.fn<DeliveryLease["release"]>>;
  };
  readonly messages: MessageSink & { readonly sendMessage: ReturnType<typeof vi.fn> };
  readonly persistedInbox: unknown[];
  readonly runtime: AgmsgRuntime;
  readonly schedulerState: SchedulerState;
  readonly ui: RuntimeContext["ui"] & {
    readonly editor: ReturnType<typeof vi.fn>;
    readonly notify: ReturnType<typeof vi.fn>;
    readonly select: ReturnType<typeof vi.fn>;
    readonly setStatus: ReturnType<typeof vi.fn>;
  };
}

const SINGLE_IDENTITY: IdentityLookup = { agent: "alice", kind: "single", teams: ["one"] };

function createClientMocks(lookup: IdentityLookup): DeliveryHarness["clientMock"] {
  return {
    inbox: vi.fn<AgmsgService["inbox"]>(async (request: InboxRequest) => `inbox:${request.team}`),
    join: vi.fn<AgmsgService["join"]>(async (_request: JoinRequest) => "joined"),
    send: vi.fn<AgmsgService["send"]>(async (request: SendRequest) => `sent:${request.team}`),
    whoami: vi.fn<AgmsgService["whoami"]>(async () => lookup),
  };
}

function createDeliveryHarness(
  lookup: IdentityLookup,
  storedIdentity?: ActiveIdentity,
  idle = false,
): DeliveryHarness {
  const clientMock = createClientMocks(lookup);
  const client: AgmsgService = {
    ...clientMock,
    history: vi.fn(async (request: HistoryRequest) => `history:${request.team}`),
    identities: vi.fn(async () => [{ agent: "alice", team: "one" }]),
    leave: vi.fn(async (request: LeaveRequest) => `left:${request.team}`),
    listTeams: vi.fn(async () => []),
    members: vi.fn(async () => [{ name: "bob", types: ["codex"] }]),
    team: vi.fn(async (team: string) => `team:${team}`),
    version: vi.fn(async () => "v1"),
  };
  const ui = {
    confirm: vi.fn(async () => true),
    editor: vi.fn(),
    notify: vi.fn(),
    select: vi.fn(async (_title: string, options: string[]) => options.at(-1)),
    setStatus: vi.fn(),
  };
  const persistedInbox: unknown[] = [];
  const pendingStates: string[][] = [[]];
  const schedulerState: SchedulerState = {
    canceled: false,
    intervalMs: undefined,
    task: undefined,
  };
  const identityStore = {
    load: vi.fn(() => storedIdentity),
    loadPending: vi.fn(() => pendingStates.at(-1) ?? []),
    save: vi.fn<IdentityStore["save"]>(),
    savePending: vi.fn<IdentityStore["savePending"]>((pending: readonly string[]): void => {
      pendingStates.push([...pending]);
    }),
  } satisfies IdentityStore;
  const messages = {
    sendMessage: vi.fn((message: { readonly content: string; readonly customType: string }) => {
      persistedInbox.push({ content: message.content, customType: message.customType });
    }),
  } satisfies MessageSink;
  const lease = {
    claim: vi.fn<DeliveryLease["claim"]>(async () => true),
    release: vi.fn<DeliveryLease["release"]>(async () => undefined),
  } satisfies DeliveryLease;
  const scheduler: RepeatScheduler = {
    repeat(task: () => void, intervalMs: number): () => void {
      schedulerState.canceled = false;
      schedulerState.intervalMs = intervalMs;
      schedulerState.task = task;
      return (): void => {
        schedulerState.canceled = true;
      };
    },
  };
  return {
    client,
    clientMock,
    context: {
      cwd: "/project",
      hasUI: true,
      isIdle: (): boolean => idle,
      sessionManager: { getEntries: (): readonly unknown[] => persistedInbox },
      signal: undefined,
      ui,
    },
    identityStore,
    lease,
    messages,
    persistedInbox,
    runtime: new AgmsgRuntime({ client, identityStore, lease, messages, scheduler }),
    schedulerState,
    ui,
  };
}

describe("automatic delivery", () => {
  it("restores a selected identity after reload without requiring reconnect", async () => {
    const harness = createDeliveryHarness(
      { agents: ["alice", "bob"], kind: "multiple", teams: ["one"] },
      { agent: "alice", teams: ["one"] },
    );
    await harness.runtime.start(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.clientMock.inbox).toHaveBeenCalledWith({
      agent: "alice",
      quiet: true,
      signal: undefined,
      team: "one",
    });
    expect(harness.ui.setStatus).toHaveBeenCalledWith("agmsg", "agmsg: alice (one)");
  });

  it("keeps a second pi process in standby while another process owns the identity", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    harness.lease.claim.mockResolvedValue(false);
    await harness.runtime.start(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.clientMock.inbox).not.toHaveBeenCalled();
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith("agmsg", "agmsg: alice (one) (standby)");
  });

  it("releases ownership while automatic delivery is disabled", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    await harness.runtime.start(harness.context);
    await harness.runtime.command("auto off", harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.lease.release).toHaveBeenCalledOnce();
    expect(harness.clientMock.inbox).not.toHaveBeenCalled();
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith("agmsg", "agmsg: alice (one) (manual)");
    await harness.runtime.command("auto on", harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.clientMock.inbox).toHaveBeenCalledOnce();
  });

  it("keeps the persisted identity when startup lookup fails transiently", async () => {
    const harness = createDeliveryHarness(
      { agents: ["alice", "bob"], kind: "multiple", teams: ["one"] },
      { agent: "alice", teams: ["one"] },
    );
    harness.clientMock.whoami.mockRejectedValueOnce(new Error("database busy"));
    await harness.runtime.start(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.clientMock.inbox).toHaveBeenCalledWith({
      agent: "alice",
      quiet: true,
      signal: undefined,
      team: "one",
    });
    expect(harness.ui.setStatus).toHaveBeenCalledWith("agmsg", "agmsg: alice (one)");
  });
});

describe("automatic message checks", () => {
  it("polls after joining and triggers a turn only for a real message", async () => {
    const harness = createDeliveryHarness({ availableTeams: [], kind: "not-joined" });
    await harness.runtime.start(harness.context);
    expect(harness.schedulerState.intervalMs).toBe(5000);
    harness.ui.editor.mockResolvedValueOnce("new-team").mockResolvedValueOnce("new-agent");
    await harness.runtime.command("setup", harness.context);
    harness.messages.sendMessage.mockClear();
    harness.clientMock.inbox.mockResolvedValue("advisor: ready");
    harness.schedulerState.task?.();
    await vi.waitFor(() => {
      expect(harness.messages.sendMessage).toHaveBeenCalledWith(
        {
          content: "Incoming agmsg message:\nadvisor: ready",
          customType: "agmsg-inbox",
          display: true,
        },
        { deliverAs: "steer", triggerTurn: true },
      );
    });
    expect(harness.clientMock.inbox).toHaveBeenCalledWith({
      agent: "new-agent",
      quiet: true,
      signal: undefined,
      team: "new-team",
    });
  });

  it("renders escaped newline codes in inbox output as readable lines", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    harness.clientMock.inbox.mockResolvedValue(String.raw`first line\n\r\nsecond paragraph`);
    await harness.runtime.start(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.messages.sendMessage).toHaveBeenCalledWith(
      {
        content: "Incoming agmsg message:\nfirst line\n\nsecond paragraph",
        customType: "agmsg-inbox",
        display: true,
      },
      { deliverAs: "steer", triggerTurn: true },
    );
  });

  it("ignores empty output, unknown identity, and stopped sessions", async () => {
    const empty = createDeliveryHarness(SINGLE_IDENTITY);
    empty.clientMock.inbox.mockResolvedValue("");
    await empty.runtime.start(empty.context);
    await empty.runtime.checkAutomatically(empty.context);
    expect(empty.messages.sendMessage).not.toHaveBeenCalled();

    const unknown = createDeliveryHarness({ availableTeams: [], kind: "not-joined" });
    await unknown.runtime.checkAutomatically(unknown.context);
    expect(unknown.clientMock.inbox).not.toHaveBeenCalled();

    await empty.runtime.stop(empty.context);
    await empty.runtime.checkAutomatically(empty.context);
    expect(empty.clientMock.inbox).toHaveBeenCalledTimes(1);
  });
});

describe("multi-team delivery", () => {
  it("delivers successful team output when another team inbox fails", async () => {
    const harness = createDeliveryHarness({
      agent: "alice",
      kind: "single",
      teams: ["one", "two"],
    });
    harness.clientMock.inbox.mockImplementation(async (request: InboxRequest): Promise<string> => {
      if (request.team === "two") {
        throw new Error("database busy");
      }
      return "message from team one";
    });
    await harness.runtime.start(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.messages.sendMessage).toHaveBeenCalledWith(
      {
        content:
          "Incoming agmsg message:\nmessage from team one\n\nInbox failed for team two: database busy",
        customType: "agmsg-inbox",
        display: true,
      },
      { deliverAs: "steer", triggerTurn: true },
    );
  });
});

describe("identity setup", () => {
  it("creates a new identity when other identities already exist", async () => {
    const harness: DeliveryHarness = createDeliveryHarness({
      agents: ["pi-developer-1", "pi-developer-2"],
      kind: "multiple",
      teams: ["dotfiles-claudex"],
    });
    harness.ui.select
      .mockResolvedValueOnce("Create a new identity…")
      .mockResolvedValueOnce("dotfiles-claudex");
    harness.ui.editor.mockResolvedValue("pi-new");
    await harness.runtime.command("setup", harness.context);
    expect(harness.clientMock.join).toHaveBeenCalledWith({
      agent: "pi-new",
      project: "/project",
      signal: undefined,
      team: "dotfiles-claudex",
    });
  });

  it("keeps existing identity selection available", async () => {
    const harness: DeliveryHarness = createDeliveryHarness({
      agents: ["alice"],
      kind: "multiple",
      teams: ["one"],
    });
    harness.ui.select.mockResolvedValue("alice");
    await harness.runtime.command("setup", harness.context);
    expect(harness.clientMock.join).not.toHaveBeenCalled();
    expect(harness.ui.setStatus).toHaveBeenCalledWith("agmsg", "agmsg: alice (one)");
  });
});

describe("sent message display", () => {
  it("shows successfully sent messages without starting an unsolicited turn", async () => {
    const harness: DeliveryHarness = createDeliveryHarness(SINGLE_IDENTITY);
    await harness.runtime.start(harness.context);
    await harness.runtime.execute({ action: "send", message: "hello", to: "bob" }, harness.context);
    expect(harness.messages.sendMessage).toHaveBeenCalledWith(
      {
        content: "Outgoing agmsg message:\nFrom: alice\nTo: bob\nTeam: one\n\nhello",
        customType: "agmsg-sent",
        display: true,
      },
      { deliverAs: "steer", triggerTurn: false },
    );
  });
});

describe("status display", () => {
  it("shows the active agent and every team name", async () => {
    const harness: DeliveryHarness = createDeliveryHarness({
      agent: "alice",
      kind: "single",
      teams: ["one", "two"],
    });
    await harness.runtime.start(harness.context);
    expect(harness.ui.setStatus).toHaveBeenCalledWith("agmsg", "agmsg: alice (one,two)");
  });
});

describe("reconnect command", () => {
  it("refreshes identity, restarts delivery, and immediately checks the inbox", async () => {
    const harness: DeliveryHarness = createDeliveryHarness(SINGLE_IDENTITY);
    await harness.runtime.start(harness.context);
    await harness.runtime.stop(harness.context);
    harness.messages.sendMessage.mockClear();

    await harness.runtime.command("reconnect", harness.context);

    expect(harness.clientMock.whoami).toHaveBeenCalledTimes(2);
    expect(harness.lease.claim).toHaveBeenCalledWith({
      force: true,
      identity: { agent: "alice", kind: "single", teams: ["one"] },
    });
    expect(harness.clientMock.inbox).toHaveBeenCalledWith({
      agent: "alice",
      quiet: true,
      signal: undefined,
      team: "one",
    });
    expect(harness.schedulerState.task).toBeTypeOf("function");
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith("agmsg", "agmsg: alice (one)");
    expect(harness.messages.sendMessage).toHaveBeenNthCalledWith(
      1,
      {
        content: "Incoming agmsg message:\ninbox:one",
        customType: "agmsg-inbox",
        display: true,
      },
      { deliverAs: "steer", triggerTurn: true },
    );
    expect(harness.messages.sendMessage).toHaveBeenNthCalledWith(2, {
      content: "Reconnected agmsg as alice in one.",
      customType: "agmsg-output",
      display: true,
    });
  });

  it("reports that reconnect requires an existing team membership", async () => {
    const harness: DeliveryHarness = createDeliveryHarness({
      availableTeams: ["one"],
      kind: "not-joined",
    });
    await harness.runtime.command("reconnect", harness.context);
    expect(harness.ui.notify).toHaveBeenCalledWith(
      "Cannot reconnect agmsg because this pi agent is not registered in any team.",
      "error",
    );
    expect(harness.ui.select).not.toHaveBeenCalled();
  });
});

describe("delivery journal", () => {
  it("retries fetched messages when pi rejects the first delivery", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    harness.clientMock.inbox.mockResolvedValueOnce("advisor: retry me").mockResolvedValueOnce("");
    harness.messages.sendMessage.mockImplementationOnce((): void => {
      throw new Error("stale extension runtime");
    });
    await harness.runtime.start(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.messages.sendMessage).toHaveBeenLastCalledWith(
      {
        content: "Incoming agmsg message:\nadvisor: retry me",
        customType: "agmsg-inbox",
        display: true,
      },
      { deliverAs: "steer", triggerTurn: true },
    );
    expect(harness.identityStore.savePending.mock.calls).toStrictEqual([
      [["advisor: retry me"]],
      [["advisor: retry me"]],
    ]);
  });

  it("waits for an in-flight inbox delivery before session shutdown", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    const deferred = Promise.withResolvers<string>();
    harness.clientMock.inbox.mockImplementationOnce(async () => deferred.promise);
    await harness.runtime.start(harness.context);
    const checking: Promise<void> = harness.runtime.checkAutomatically(harness.context);
    const stopping: Promise<void> = harness.runtime.stop(harness.context);
    expect(harness.schedulerState.canceled).toBe(false);
    deferred.resolve("message during reload");
    await checking;
    await stopping;
    expect(harness.schedulerState.canceled).toBe(true);
    expect(harness.messages.sendMessage).toHaveBeenCalledWith(
      {
        content: "Incoming agmsg message:\nmessage during reload",
        customType: "agmsg-inbox",
        display: true,
      },
      { deliverAs: "steer", triggerTurn: true },
    );
  });
});

describe("automatic delivery resilience", () => {
  it("coalesces overlapping checks so burst messages are not delayed or missed", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    const deferred = Promise.withResolvers<string>();
    harness.clientMock.inbox
      .mockImplementationOnce(async () => deferred.promise)
      .mockResolvedValueOnce("second burst message");
    await harness.runtime.start(harness.context);
    const first = harness.runtime.checkAutomatically(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    deferred.resolve("first burst message");
    await first;
    expect(harness.clientMock.inbox).toHaveBeenCalledTimes(2);
    expect(harness.messages.sendMessage).toHaveBeenNthCalledWith(
      2,
      {
        content: "Incoming agmsg message:\nsecond burst message",
        customType: "agmsg-inbox",
        display: true,
      },
      { deliverAs: "steer", triggerTurn: true },
    );
  });

  it("surfaces script failures as status without throwing", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    harness.clientMock.inbox.mockRejectedValue(new Error("database busy"));
    await harness.runtime.start(harness.context);
    await expect(harness.runtime.checkAutomatically(harness.context)).resolves.toBeUndefined();
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith(
      "agmsg",
      "agmsg: Inbox failed for team one: database busy",
    );
  });

  it("requires setup for tool calls and tolerates startup lookup failures", async () => {
    const missing = createDeliveryHarness({ availableTeams: [], kind: "not-joined" });
    await expect(missing.runtime.execute({ action: "inbox" }, missing.context)).rejects.toThrow(
      "Run /agmsg setup",
    );

    const failed = createDeliveryHarness(SINGLE_IDENTITY);
    failed.clientMock.whoami.mockRejectedValue(new Error("missing agmsg"));
    await expect(failed.runtime.start(failed.context)).resolves.toBeUndefined();
    expect(failed.identityStore.save).not.toHaveBeenCalled();
    expect(failed.ui.setStatus).toHaveBeenCalledWith("agmsg", undefined);
  });
});
