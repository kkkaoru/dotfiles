import { describe, expect, it, vi } from "vitest";
import type {
  AgmsgService,
  HistoryRequest,
  IdentityLookup,
  InboxRequest,
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
    readonly send: ReturnType<typeof vi.fn<AgmsgService["send"]>>;
    readonly whoami: ReturnType<typeof vi.fn<AgmsgService["whoami"]>>;
  };
  readonly context: RuntimeContext;
  readonly messages: MessageSink & { readonly sendMessage: ReturnType<typeof vi.fn> };
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

function createDeliveryHarness(lookup: IdentityLookup): DeliveryHarness {
  const inbox = vi.fn<AgmsgService["inbox"]>(
    async (request: InboxRequest) => `inbox:${request.team}`,
  );
  const send = vi.fn<AgmsgService["send"]>(async (request: SendRequest) => `sent:${request.team}`);
  const whoami = vi.fn<AgmsgService["whoami"]>(async () => lookup);
  const client: AgmsgService = {
    history: vi.fn(async (request: HistoryRequest) => `history:${request.team}`),
    identities: vi.fn(async () => [{ agent: "alice", team: "one" }]),
    inbox,
    join: vi.fn(async (_request: JoinRequest) => "joined"),
    leave: vi.fn(async (request: LeaveRequest) => `left:${request.team}`),
    listTeams: vi.fn(async () => []),
    members: vi.fn(async () => [{ name: "bob", types: ["codex"] }]),
    send,
    team: vi.fn(async (team: string) => `team:${team}`),
    version: vi.fn(async () => "v1"),
    whoami,
  };
  const ui = {
    confirm: vi.fn(async () => true),
    editor: vi.fn(),
    notify: vi.fn(),
    select: vi.fn(async (_title: string, options: string[]) => options.at(-1)),
    setStatus: vi.fn(),
  };
  const context: RuntimeContext = { cwd: "/project", hasUI: true, signal: undefined, ui };
  const messages = { sendMessage: vi.fn() } satisfies MessageSink;
  const schedulerState: SchedulerState = {
    canceled: false,
    intervalMs: undefined,
    task: undefined,
  };
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
    clientMock: { inbox, send, whoami },
    context,
    messages,
    runtime: new AgmsgRuntime(messages, client, scheduler),
    schedulerState,
    ui,
  };
}

describe("automatic delivery", () => {
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

    empty.runtime.stop(empty.context);
    await empty.runtime.checkAutomatically(empty.context);
    expect(empty.clientMock.inbox).toHaveBeenCalledTimes(1);
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
    harness.runtime.stop(harness.context);
    harness.messages.sendMessage.mockClear();

    await harness.runtime.command("reconnect", harness.context);

    expect(harness.clientMock.whoami).toHaveBeenCalledTimes(2);
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

describe("automatic delivery resilience", () => {
  it("prevents overlapping background and turn checks", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    const deferred = Promise.withResolvers<string>();
    harness.clientMock.inbox.mockImplementation(async () => deferred.promise);
    await harness.runtime.start(harness.context);
    const first = harness.runtime.checkAutomatically(harness.context);
    await harness.runtime.checkAutomatically(harness.context);
    deferred.resolve("");
    await first;
    expect(harness.clientMock.inbox).toHaveBeenCalledTimes(1);
  });

  it("surfaces script failures as status without throwing", async () => {
    const harness = createDeliveryHarness(SINGLE_IDENTITY);
    harness.clientMock.inbox.mockRejectedValue(new Error("database busy"));
    await harness.runtime.start(harness.context);
    await expect(harness.runtime.checkAutomatically(harness.context)).resolves.toBeUndefined();
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith("agmsg", "agmsg: database busy");
  });

  it("requires setup for tool calls and tolerates startup lookup failures", async () => {
    const missing = createDeliveryHarness({ availableTeams: [], kind: "not-joined" });
    await expect(missing.runtime.execute({ action: "inbox" }, missing.context)).rejects.toThrow(
      "Run /agmsg setup",
    );

    const failed = createDeliveryHarness(SINGLE_IDENTITY);
    failed.clientMock.whoami.mockRejectedValue(new Error("missing agmsg"));
    await expect(failed.runtime.start(failed.context)).resolves.toBeUndefined();
    expect(failed.ui.setStatus).toHaveBeenCalledWith("agmsg", undefined);
  });
});
