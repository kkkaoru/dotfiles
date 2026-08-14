import { describe, expect, it, vi } from "vitest";
import agmsgExtension, { type AgmsgExtensionHost } from "./index.ts";
import type { RuntimeContext } from "./src/contracts.ts";

interface RegistrationState {
  command: Parameters<AgmsgExtensionHost["registerCommand"]>[1] | undefined;
  tool: Parameters<AgmsgExtensionHost["registerTool"]>[0] | undefined;
}

interface ExtensionHarness {
  readonly command: () => NonNullable<RegistrationState["command"]>;
  readonly context: RuntimeContext;
  readonly events: Readonly<
    Record<string, ((event: unknown, context: RuntimeContext) => Promise<void> | void) | undefined>
  >;
  readonly host: AgmsgExtensionHost;
  readonly hostMocks: {
    readonly exec: ReturnType<typeof vi.fn<AgmsgExtensionHost["exec"]>>;
    readonly registerCommand: ReturnType<typeof vi.fn<AgmsgExtensionHost["registerCommand"]>>;
    readonly registerTool: ReturnType<typeof vi.fn<AgmsgExtensionHost["registerTool"]>>;
    readonly sendMessage: ReturnType<typeof vi.fn<AgmsgExtensionHost["sendMessage"]>>;
  };
  readonly tool: () => NonNullable<RegistrationState["tool"]>;
}

function scriptResult(scriptPath: string): string {
  if (scriptPath.endsWith("whoami.sh")) {
    return "agent=alice teams=one type=pi project=/project\n";
  }
  if (scriptPath.endsWith("team.sh")) {
    return "x".repeat(60_000);
  }
  return scriptPath.endsWith("inbox.sh") ? "" : "ok\n";
}

function required<Value>(value: Value | undefined, message: string): Value {
  if (value === undefined) {
    throw new Error(message);
  }
  return value;
}

function createExtensionHarness(): ExtensionHarness {
  const state: RegistrationState = { command: undefined, tool: undefined };
  const events: Record<
    string,
    ((event: unknown, context: RuntimeContext) => Promise<void> | void) | undefined
  > = {};
  const exec = vi.fn<AgmsgExtensionHost["exec"]>(async (_command: string, args: string[]) => ({
    code: 0,
    killed: false,
    stderr: "",
    stdout: scriptResult(args[0] ?? ""),
  }));
  const registerCommand = vi.fn<AgmsgExtensionHost["registerCommand"]>((_name, definition) => {
    state.command = definition;
  });
  const registerTool = vi.fn<AgmsgExtensionHost["registerTool"]>((definition) => {
    state.tool = definition;
  });
  const sendMessage = vi.fn<AgmsgExtensionHost["sendMessage"]>();
  const host: AgmsgExtensionHost = {
    exec,
    on(event, handler): void {
      events[event] = handler;
    },
    registerCommand,
    registerTool,
    sendMessage,
  };
  const context: RuntimeContext = {
    cwd: "/project",
    hasUI: true,
    signal: undefined,
    ui: {
      confirm: vi.fn(async () => true),
      editor: vi.fn(),
      notify: vi.fn(),
      select: vi.fn(),
      setStatus: vi.fn(),
    },
  };
  agmsgExtension(host);
  return {
    command: (): NonNullable<RegistrationState["command"]> =>
      required(state.command, "command not registered"),
    context,
    events,
    host,
    hostMocks: { exec, registerCommand, registerTool, sendMessage },
    tool: (): NonNullable<RegistrationState["tool"]> => required(state.tool, "tool not registered"),
  };
}

describe("pi extension registration", () => {
  it("registers command, tool, and lifecycle events", () => {
    const harness: ExtensionHarness = createExtensionHarness();
    expect(harness.hostMocks.registerTool).toHaveBeenCalledOnce();
    expect(harness.hostMocks.registerCommand).toHaveBeenCalledOnce();
    expect(harness.hostMocks.registerCommand.mock.calls[0]?.[0]).toBe("agmsg");
    expect(Object.keys(harness.events)).toStrictEqual([
      "session_start",
      "agent_settled",
      "session_shutdown",
    ]);
    expect(harness.tool().name).toBe("agmsg");
  });

  it("executes tool actions and truncates oversized output", async () => {
    const harness: ExtensionHarness = createExtensionHarness();
    const tool: ReturnType<ExtensionHarness["tool"]> = harness.tool();
    const whoami: Awaited<ReturnType<typeof tool.execute>> = await tool.execute(
      "call",
      { action: "whoami" },
      undefined,
      undefined,
      harness.context,
    );
    expect(whoami.content[0]).toMatchObject({ text: expect.stringContaining("agent=alice") });
    expect(whoami.details).toStrictEqual({ action: "whoami", truncated: false });

    const team: Awaited<ReturnType<typeof tool.execute>> = await tool.execute(
      "call",
      { action: "team" },
      undefined,
      undefined,
      harness.context,
    );
    expect(team.content[0]).toMatchObject({ text: expect.stringContaining("output truncated") });
    expect(team.details).toStrictEqual({ action: "team", truncated: true });
  });

  it("offers completions and dispatches commands", async () => {
    const harness: ExtensionHarness = createExtensionHarness();
    const command: ReturnType<ExtensionHarness["command"]> = harness.command();
    expect(command.getArgumentCompletions("hi")).toStrictEqual([
      { label: "history", value: "history" },
    ]);
    expect(command.getArgumentCompletions("rec")).toStrictEqual([
      { label: "reconnect", value: "reconnect" },
    ]);
    expect(command.getArgumentCompletions("zzz")).toBeNull();
    await command.handler("help", harness.context);
    expect(harness.hostMocks.sendMessage).toHaveBeenCalledWith(
      expect.objectContaining({ content: expect.stringContaining("agmsg commands") }),
    );
  });

  it("runs lifecycle callbacks", async () => {
    const harness: ExtensionHarness = createExtensionHarness();
    await harness.events["session_start"]?.({}, harness.context);
    await harness.events["agent_settled"]?.({}, harness.context);
    await harness.events["session_shutdown"]?.({}, harness.context);
    expect(harness.hostMocks.exec).toHaveBeenCalled();
    expect(harness.context.ui.setStatus).toHaveBeenLastCalledWith("agmsg", undefined);
  });
});
