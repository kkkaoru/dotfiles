import { describe, expect, it, vi } from "vitest";
import type {
  AgmsgService,
  HistoryRequest,
  IdentityLookup,
  InboxRequest,
  LeaveRequest,
  MessageSink,
  RepeatScheduler,
  RuntimeContext,
  SendRequest,
} from "./contracts.ts";
import { AgmsgRuntime } from "./runtime.ts";

interface ClientMock {
  readonly history: ReturnType<typeof vi.fn>;
  readonly identities: ReturnType<typeof vi.fn>;
  readonly inbox: ReturnType<typeof vi.fn>;
  readonly join: ReturnType<typeof vi.fn>;
  readonly leave: ReturnType<typeof vi.fn>;
  readonly listTeams: ReturnType<typeof vi.fn>;
  readonly members: ReturnType<typeof vi.fn>;
  readonly send: ReturnType<typeof vi.fn>;
  readonly team: ReturnType<typeof vi.fn>;
  readonly version: ReturnType<typeof vi.fn>;
  readonly whoami: ReturnType<typeof vi.fn>;
}

interface SchedulerState {
  canceled: boolean;
  task: (() => void) | undefined;
}

interface Harness {
  readonly client: AgmsgService;
  readonly clientMock: ClientMock;
  readonly commandContext: RuntimeContext;
  readonly context: RuntimeContext;
  readonly piMock: MessageSink & {
    readonly sendMessage: ReturnType<typeof vi.fn>;
  };
  readonly runtime: AgmsgRuntime;
  readonly schedulerState: SchedulerState;
  readonly ui: {
    readonly confirm: ReturnType<typeof vi.fn>;
    readonly editor: ReturnType<typeof vi.fn>;
    readonly notify: ReturnType<typeof vi.fn>;
    readonly select: ReturnType<typeof vi.fn>;
    readonly setStatus: ReturnType<typeof vi.fn>;
  };
}

const single: IdentityLookup = { agent: "alice", kind: "single", teams: ["one"] };

function createHarness(lookup?: IdentityLookup): Harness {
  const resolvedLookup: IdentityLookup = lookup ?? single;
  const clientMock = {
    history: vi.fn(async (request: HistoryRequest) => `history:${request.team}`),
    identities: vi.fn(async () => [{ agent: "alice", team: "one" }]),
    inbox: vi.fn(async (request: InboxRequest) => `inbox:${request.team}`),
    join: vi.fn(async () => "joined"),
    leave: vi.fn(async (request: LeaveRequest) => `left:${request.team}`),
    listTeams: vi.fn(async () => []),
    members: vi.fn(async () => [{ name: "bob", types: ["codex"] }]),
    send: vi.fn(async (request: SendRequest) => `sent:${request.team}`),
    team: vi.fn(async (team: string) => `team:${team}`),
    version: vi.fn(async () => "v1"),
    whoami: vi.fn(async () => resolvedLookup),
  } satisfies AgmsgService;
  const client: AgmsgService = clientMock;
  const ui = {
    confirm: vi.fn(async () => true),
    editor: vi.fn(),
    notify: vi.fn(),
    select: vi.fn(async (_title: string, options: readonly string[]) => options.at(-1)),
    setStatus: vi.fn(),
  };
  const context: RuntimeContext = {
    cwd: "/project",
    hasUI: true,
    signal: undefined,
    ui,
  };
  const commandContext: RuntimeContext = context;
  const piMock = { sendMessage: vi.fn() } satisfies MessageSink;
  const schedulerState: SchedulerState = { canceled: false, task: undefined };
  const scheduler: RepeatScheduler = {
    repeat(task: () => void): () => void {
      schedulerState.canceled = false;
      schedulerState.task = task;
      return (): void => {
        schedulerState.canceled = true;
      };
    },
  };
  return {
    client,
    clientMock,
    commandContext,
    context,
    piMock,
    runtime: new AgmsgRuntime(piMock, client, scheduler),
    schedulerState,
    ui,
  };
}

async function start(harness: Harness): Promise<void> {
  await harness.runtime.start(harness.context);
}

describe("AgmsgRuntime lifecycle and tool actions", () => {
  it("sets and clears status", async () => {
    const harness: Harness = createHarness();
    await start(harness);
    expect(harness.ui.setStatus).toHaveBeenCalledWith("agmsg", "agmsg: alice (one)");
    expect(harness.schedulerState.task).toBeTypeOf("function");
    harness.runtime.stop(harness.context);
    expect(harness.schedulerState.canceled).toBe(true);
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith("agmsg", undefined);
  });

  it("runs inbox, history, team, and whoami actions", async () => {
    const harness: Harness = createHarness();
    await start(harness);
    await expect(harness.runtime.execute({ action: "inbox" }, harness.context)).resolves.toBe(
      "inbox:one",
    );
    await expect(
      harness.runtime.execute({ action: "history", limit: 5, team: "one" }, harness.context),
    ).resolves.toBe("history:one");
    await expect(
      harness.runtime.execute({ action: "team", team: "one" }, harness.context),
    ).resolves.toBe("team:one");
    await expect(harness.runtime.execute({ action: "whoami" }, harness.context)).resolves.toBe(
      "agent=alice teams=one type=pi project=/project",
    );
    expect(harness.clientMock.history).toHaveBeenCalledWith({
      agent: "alice",
      limit: 5,
      signal: undefined,
      team: "one",
    });
  });

  it("sends to the only active team", async () => {
    const harness: Harness = createHarness();
    await start(harness);
    await expect(
      harness.runtime.execute({ action: "send", message: "hello", to: "bob" }, harness.context),
    ).resolves.toBe("sent:one");
    expect(harness.clientMock.send).toHaveBeenCalledWith({
      from: "alice",
      message: "hello",
      signal: undefined,
      team: "one",
      to: "bob",
    });
  });

  it("validates tool input and team membership", async () => {
    const harness: Harness = createHarness();
    await start(harness);
    await expect(
      harness.runtime.execute({ action: "send", to: "bob" }, harness.context),
    ).rejects.toThrow("requires both");
    await expect(
      harness.runtime.execute({ action: "team", team: "other" }, harness.context),
    ).rejects.toThrow("not in team");
  });
});

describe("target team resolution", () => {
  it("honors an explicit team", async () => {
    const harness = createHarness({ agent: "alice", kind: "single", teams: ["one", "two"] });
    await start(harness);
    await harness.runtime.execute(
      { action: "send", message: "hi", team: "two", to: "bob" },
      harness.context,
    );
    expect(harness.clientMock.send).toHaveBeenCalledWith({
      from: "alice",
      message: "hi",
      signal: undefined,
      team: "two",
      to: "bob",
    });
  });

  it("finds a unique target across teams", async () => {
    const harness = createHarness({ agent: "alice", kind: "single", teams: ["one", "two"] });
    harness.clientMock.members.mockImplementation(async (team: string) =>
      team === "two" ? [{ name: "bob", types: [] }] : [],
    );
    await start(harness);
    await harness.runtime.execute({ action: "send", message: "hi", to: "bob" }, harness.context);
    expect(harness.clientMock.send).toHaveBeenCalledWith({
      from: "alice",
      message: "hi",
      signal: undefined,
      team: "two",
      to: "bob",
    });
  });

  it("rejects ambiguous and missing targets", async () => {
    const harness = createHarness({ agent: "alice", kind: "single", teams: ["one", "two"] });
    await start(harness);
    await expect(
      harness.runtime.execute({ action: "send", message: "hi", to: "bob" }, harness.context),
    ).rejects.toThrow("matched 2 teams");
    harness.clientMock.members.mockResolvedValue([]);
    await expect(
      harness.runtime.execute({ action: "send", message: "hi", to: "bob" }, harness.context),
    ).rejects.toThrow("matched 0 teams");
  });
});

describe("slash command", () => {
  it("displays help, version, identity, team, history, send, and inbox", async () => {
    const harness = createHarness();
    await start(harness);
    const commands: readonly string[] = [
      "help",
      "version",
      "whoami",
      "team",
      "history 3",
      "send bob hello",
      "",
    ];
    await Promise.all(
      commands.map(async (command: string): Promise<void> =>
        harness.runtime.command(command, harness.commandContext),
      ),
    );
    expect(harness.piMock.sendMessage).toHaveBeenCalledTimes(7);
    expect(harness.clientMock.history).toHaveBeenCalledWith({
      agent: "alice",
      limit: 3,
      signal: undefined,
      team: "one",
    });
    expect(harness.clientMock.send).toHaveBeenCalledWith({
      from: "alice",
      message: "hello",
      signal: undefined,
      team: "one",
      to: "bob",
    });
  });

  it("reports bad commands, arguments, and history limits", async () => {
    const harness = createHarness();
    await start(harness);
    const commands: readonly string[] = ["unknown", "send bob", "history 0", "auto maybe"];
    await Promise.all(
      commands.map(async (command: string): Promise<void> =>
        harness.runtime.command(command, harness.commandContext),
      ),
    );
    expect(harness.ui.notify).toHaveBeenCalledTimes(4);
    expect(harness.ui.notify).toHaveBeenCalledWith("Usage: /agmsg auto <on|off>", "error");
  });

  it("toggles automatic delivery", async () => {
    const harness = createHarness();
    await start(harness);
    await harness.runtime.command("auto off", harness.commandContext);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.clientMock.inbox).not.toHaveBeenCalled();
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith("agmsg", "agmsg: alice (one) (manual)");
    await harness.runtime.command("auto on", harness.commandContext);
    await harness.runtime.checkAutomatically(harness.context);
    expect(harness.clientMock.inbox).toHaveBeenCalled();
  });
});

describe("leave command", () => {
  it("leaves the only active team after confirmation", async () => {
    const harness = createHarness();
    await start(harness);
    await harness.runtime.command("leave", harness.commandContext);
    expect(harness.ui.confirm).toHaveBeenCalledWith(
      "Leave agmsg team?",
      "alice will leave one across all registered projects.",
    );
    expect(harness.clientMock.leave).toHaveBeenCalledWith({
      agent: "alice",
      signal: undefined,
      team: "one",
    });
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith("agmsg", undefined);
    expect(harness.piMock.sendMessage).toHaveBeenCalledWith(
      expect.objectContaining({ content: "left:one" }),
    );
  });

  it("supports team selection and cancellation", async () => {
    const harness = createHarness({ agent: "alice", kind: "single", teams: ["one", "two"] });
    harness.ui.select.mockResolvedValue("two");
    await start(harness);
    await harness.runtime.command("leave", harness.commandContext);
    expect(harness.clientMock.leave).toHaveBeenCalledWith({
      agent: "alice",
      signal: undefined,
      team: "two",
    });
    expect(harness.ui.setStatus).toHaveBeenLastCalledWith("agmsg", "agmsg: alice (one)");

    const cancelled = createHarness();
    cancelled.ui.confirm.mockResolvedValue(false);
    await start(cancelled);
    await cancelled.runtime.command("leave one", cancelled.commandContext);
    expect(cancelled.clientMock.leave).not.toHaveBeenCalled();
    expect(cancelled.piMock.sendMessage).toHaveBeenCalledWith(
      expect.objectContaining({ content: "Leave cancelled." }),
    );
  });

  it("throws instead of starting setup when no team membership exists", async () => {
    const harness = createHarness({ availableTeams: ["one"], kind: "not-joined" });
    await expect(harness.runtime.command("leave", harness.commandContext)).rejects.toThrow(
      "Cannot leave an agmsg team because this pi agent is not registered in any team.",
    );
    expect(harness.clientMock.leave).not.toHaveBeenCalled();
    expect(harness.ui.select).not.toHaveBeenCalled();
  });

  it("validates explicit teams and requires confirmation UI", async () => {
    const invalid = createHarness();
    await start(invalid);
    await expect(invalid.runtime.command("leave missing", invalid.commandContext)).rejects.toThrow(
      "Identity alice is not in team missing.",
    );

    const headless = createHarness();
    Object.assign(headless.commandContext, { hasUI: false });
    await start(headless);
    await expect(headless.runtime.command("leave one", headless.commandContext)).rejects.toThrow(
      "Leaving an agmsg team requires TUI or RPC confirmation.",
    );
  });
});

describe("identity setup", () => {
  it("selects and joins an existing identity", async () => {
    const harness = createHarness({ availableTeams: ["one"], kind: "not-joined" });
    harness.clientMock.listTeams.mockResolvedValue(["one", "two"]);
    harness.ui.select.mockResolvedValue("one");
    harness.ui.editor.mockResolvedValueOnce(" alice ");
    await harness.runtime.command("", harness.commandContext);
    expect(harness.ui.select).toHaveBeenCalledWith("Choose an existing agmsg team or create one", [
      "one",
      "two",
      "Create a new team…",
    ]);
    expect(harness.clientMock.join).toHaveBeenCalledWith({
      agent: "alice",
      project: "/project",
      signal: undefined,
      team: "one",
    });
    expect(harness.clientMock.inbox).not.toHaveBeenCalled();
    expect(harness.piMock.sendMessage).toHaveBeenCalledWith({
      content:
        "joined\nAutomatic background and end-of-turn delivery is enabled for this pi session.",
      customType: "agmsg-output",
      display: true,
    });
  });

  it("supports explicit setup for suggestions", async () => {
    const harness = createHarness({
      agents: ["old"],
      availableTeams: ["one"],
      kind: "suggestion",
      teams: ["one"],
    });
    harness.ui.editor.mockResolvedValueOnce("two").mockResolvedValueOnce("new");
    await harness.runtime.command("setup", harness.commandContext);
    expect(harness.clientMock.join).toHaveBeenCalled();
    expect(harness.piMock.sendMessage).toHaveBeenCalledWith({
      content:
        "joined\nAutomatic background and end-of-turn delivery is enabled for this pi session.",
      customType: "agmsg-output",
      display: true,
    });
  });

  it("selects one of multiple identities", async () => {
    const harness = createHarness({
      agents: ["alice", "bob"],
      kind: "multiple",
      teams: ["one", "two"],
    });
    harness.clientMock.identities.mockResolvedValue([
      { agent: "alice", team: "one" },
      { agent: "alice", team: "two" },
      { agent: "bob", team: "two" },
    ]);
    harness.ui.select.mockResolvedValue("alice");
    await harness.runtime.command("whoami", harness.commandContext);
    expect(harness.piMock.sendMessage).toHaveBeenCalledWith(
      expect.objectContaining({ content: "agent=alice teams=one,two type=pi" }),
    );
  });
});

describe("identity setup errors", () => {
  it("reports setup cancellation and unavailable UI", async () => {
    const cancelled = createHarness({ availableTeams: [], kind: "not-joined" });
    cancelled.ui.select.mockResolvedValue(undefined);
    await cancelled.runtime.command("", cancelled.commandContext);
    expect(cancelled.ui.notify).toHaveBeenCalledWith("Team selection cancelled", "error");

    const headless = createHarness({ agents: ["a", "b"], kind: "multiple", teams: ["one"] });
    Object.assign(headless.commandContext, { hasUI: false });
    await headless.runtime.command("", headless.commandContext);
    expect(headless.ui.notify).toHaveBeenCalledWith(expect.stringContaining("TUI mode"), "error");
  });

  it("reports agent and identity selection cancellation", async () => {
    const harness = createHarness({ availableTeams: [], kind: "not-joined" });
    harness.ui.editor.mockResolvedValueOnce("one").mockResolvedValueOnce(undefined);
    await harness.runtime.command("setup", harness.commandContext);
    expect(harness.ui.notify).toHaveBeenCalledWith("Agent selection cancelled", "error");

    const multiple = createHarness({ agents: ["a", "b"], kind: "multiple", teams: ["one"] });
    multiple.ui.select.mockResolvedValue(undefined);
    await multiple.runtime.command("setup", multiple.commandContext);
    expect(multiple.ui.notify).toHaveBeenCalledWith("Identity selection cancelled", "error");
  });
});

describe("team creation flow", () => {
  it("rejects cancellation and an existing name in create mode", async () => {
    const cancelled = createHarness({ availableTeams: [], kind: "not-joined" });
    cancelled.ui.editor.mockResolvedValue(undefined);
    await cancelled.runtime.command("setup", cancelled.commandContext);
    expect(cancelled.ui.notify).toHaveBeenCalledWith("Team creation cancelled", "error");

    const duplicate = createHarness({ availableTeams: ["one"], kind: "not-joined" });
    duplicate.ui.editor.mockResolvedValue("one");
    await duplicate.runtime.command("setup", duplicate.commandContext);
    expect(duplicate.ui.notify).toHaveBeenCalledWith(
      "Team one already exists; select it from the team list.",
      "error",
    );
  });
});

describe("identity setup defaults", () => {
  it("uses the directory and a unique random name as blank-input defaults", async () => {
    const harness = createHarness({ availableTeams: [], kind: "not-joined" });
    Object.assign(harness.commandContext, {
      cwd: "/Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles",
    });
    harness.clientMock.members.mockResolvedValue([{ name: "someone-else", types: [] }]);
    harness.ui.editor.mockResolvedValueOnce("").mockResolvedValueOnce("");
    await harness.runtime.command("setup", harness.commandContext);
    expect(harness.ui.editor).toHaveBeenNthCalledWith(1, "New agmsg team name", "dotfiles");
    expect(harness.ui.editor).toHaveBeenNthCalledWith(
      2,
      "pi agent name",
      expect.stringMatching(/^pi-[a-f\d]{12}$/u),
    );
    expect(harness.clientMock.members).toHaveBeenCalledWith("dotfiles", undefined);
    expect(harness.clientMock.join).toHaveBeenCalledWith({
      agent: expect.stringMatching(/^pi-[a-f\d]{12}$/u),
      project: "/Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles",
      signal: undefined,
      team: "dotfiles",
    });
  });

  it("can set up from an existing single identity and rejects headless setup", async () => {
    const existing = createHarness();
    existing.ui.editor.mockResolvedValueOnce("two").mockResolvedValueOnce("alice-two");
    await existing.runtime.command("setup", existing.commandContext);
    expect(existing.clientMock.join).toHaveBeenCalledWith({
      agent: "alice-two",
      project: "/project",
      signal: undefined,
      team: "two",
    });

    const headless = createHarness({ availableTeams: [], kind: "not-joined" });
    Object.assign(headless.commandContext, { hasUI: false });
    await headless.runtime.command("setup", headless.commandContext);
    expect(headless.ui.notify).toHaveBeenCalledWith(
      "agmsg setup requires TUI or RPC mode",
      "error",
    );
  });
});
