// This file runs with Bun.
import { PassThrough } from "node:stream";
import type { SessionUpdate } from "@agentclientprotocol/sdk";
import { beforeEach, expect, test, vi } from "vitest";

interface FakeChild {
  exitCode: number | null;
  stderr: PassThrough;
  stdin: PassThrough;
  stdout: PassThrough;
  kill: ReturnType<typeof vi.fn>;
  once: ReturnType<typeof vi.fn>;
  unref: ReturnType<typeof vi.fn>;
}

interface FakeSession {
  dispose: ReturnType<typeof vi.fn>;
  modes:
    | {
        availableModes: Array<{ id: string; name: string }>;
        currentModeId: string;
      }
    | undefined;
  nextUpdate: () => Promise<unknown>;
  prompt: ReturnType<typeof vi.fn>;
  sessionId: string;
}

interface FakeContext {
  buildSession: (request: unknown) => {
    start: () => Promise<FakeSession>;
    withSession: (operation: (session: FakeSession) => Promise<void>) => Promise<void>;
  };
  notify: ReturnType<typeof vi.fn>;
  request: ReturnType<typeof vi.fn>;
}

interface Scenario {
  connectError: Error | undefined;
  modes: FakeSession["modes"];
  setModeError: Error | undefined;
  stderr: string;
  turnUpdates: unknown[][];
}

interface MockState {
  children: FakeChild[];
  deleteCalls: unknown[];
  permissionHandler: ((context: { params: unknown }) => unknown) | undefined;
  scenarioIndex: number;
  scenarios: Scenario[];
  sessionRequests: unknown[];
  sessions: FakeSession[];
  spawnIndex: number;
}

interface AcpReader {
  cancel: () => Promise<void>;
  read: () => Promise<unknown>;
}

interface AcpReadable {
  getReader: () => AcpReader;
}

interface JobInput {
  continuationPrompt: string;
  cwd: string;
  initialPrompt: string;
  modelId: string;
  onUpdate: (update: SessionUpdate) => void;
  sessionId: string;
  signal: AbortSignal | undefined;
}

const mocks = vi.hoisted(() => ({
  client: vi.fn(),
  ndJsonStream: vi.fn<
    (output: unknown, input: AcpReadable) => { readable: object; writable: object }
  >(() => ({ readable: {}, writable: {} })),
  spawn: vi.fn(),
}));

const state: MockState = {
  children: [],
  deleteCalls: [],
  permissionHandler: undefined,
  scenarios: [],
  scenarioIndex: 0,
  sessionRequests: [],
  sessions: [],
  spawnIndex: 0,
};

vi.mock("node:child_process", async (importOriginal) => {
  const original = await importOriginal<typeof import("node:child_process")>();
  return { ...original, spawn: mocks.spawn };
});

vi.mock("@agentclientprotocol/sdk", async (importOriginal) => {
  const original = await importOriginal<typeof import("@agentclientprotocol/sdk")>();
  return { ...original, client: mocks.client, ndJsonStream: mocks.ndJsonStream };
});

function createChild(stderr: string): FakeChild {
  const listeners: { error?: () => void; exit?: () => void } = {};
  const child: FakeChild = {
    exitCode: null,
    stdin: new PassThrough(),
    stdout: new PassThrough(),
    stderr: new PassThrough(),
    kill: vi.fn(),
    once: vi.fn((event: string, handler: () => void) => {
      if (event === "exit") listeners.exit = handler;
      if (event === "error") listeners.error = handler;
    }),
    unref: vi.fn(),
  };
  child.kill.mockImplementation(() => {
    child.exitCode = 0;
    child.stdout.end();
    listeners.exit?.();
    return true;
  });
  if (stderr.length > 0) queueMicrotask(() => child.stderr.end(stderr));
  return child;
}

function installScenario(scenario: Scenario): void {
  state.scenarios.push(scenario);
}

function textUpdate(text: string): unknown {
  return {
    kind: "session_update",
    update: {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text },
    },
  };
}

function createSession(scenario: Scenario): FakeSession {
  const turnState: { index: number; updates: unknown[] } = {
    index: 0,
    updates: [],
  };
  const session: FakeSession = {
    sessionId: `session-${state.sessions.length + 1}`,
    modes: scenario.modes,
    dispose: vi.fn(),
    prompt: vi.fn(async () => {
      const turnIndex = session.prompt.mock.calls.length - 1;
      turnState.updates = scenario.turnUpdates[turnIndex] ?? [];
      turnState.index = 0;
      return { stopReason: "end_turn" };
    }),
    nextUpdate: async () => {
      const update = turnState.updates[turnState.index];
      turnState.index += 1;
      return update ?? { kind: "stop", response: { stopReason: "end_turn" } };
    },
  };
  state.sessions.push(session);
  return session;
}

function configureAcpMock(): void {
  mocks.spawn.mockImplementation(() => {
    const scenario = state.scenarios[state.spawnIndex];
    state.spawnIndex += 1;
    const child = createChild(scenario?.stderr ?? "");
    state.children.push(child);
    return child;
  });
  mocks.client.mockImplementation(() => {
    const app = {
      onRequest: vi.fn(),
      connectWith: vi.fn(),
    };
    app.onRequest.mockImplementation((_method, handler) => {
      state.permissionHandler = handler;
      return app;
    });
    app.connectWith.mockImplementation(async (_stream, operation) => {
      const scenario = state.scenarios[state.scenarioIndex];
      state.scenarioIndex += 1;
      if (!scenario) throw new Error("missing scenario");
      if (scenario.connectError) throw scenario.connectError;
      const context: FakeContext = {
        request: vi.fn(async (method: string, params: unknown) => {
          if (method === "session/set_mode" && scenario.setModeError) {
            throw scenario.setModeError;
          }
          if (method === "session/delete") {
            state.deleteCalls.push(params);
          }
          return { protocolVersion: 1 };
        }),
        notify: vi.fn(async () => undefined),
        buildSession: (request) => {
          state.sessionRequests.push(request);
          return {
            start: async () => createSession(scenario),
            withSession: async (sessionOperation) => {
              const session = createSession(scenario);
              await sessionOperation(session);
            },
          };
        },
      };
      await operation(context);
    });
    return app;
  });
}

function job(input: {
  continuationPrompt?: string;
  cwd?: string;
  initialPrompt: string;
  modelId?: string;
  onUpdate?: JobInput["onUpdate"];
  sessionId: string;
  signal?: AbortSignal;
}): JobInput {
  return {
    continuationPrompt: input.continuationPrompt ?? input.initialPrompt,
    cwd: input.cwd ?? "/tmp/project-a",
    initialPrompt: input.initialPrompt,
    modelId: input.modelId ?? "swe-1-7",
    onUpdate: input.onUpdate ?? (() => undefined),
    sessionId: input.sessionId,
    signal: input.signal,
  };
}

beforeEach(async () => {
  process.argv = process.argv.filter((arg) => arg !== "-p" && arg !== "--print");
  state.children = [];
  state.deleteCalls = [];
  state.permissionHandler = undefined;
  state.scenarios = [];
  state.scenarioIndex = 0;
  state.sessionRequests = [];
  state.sessions = [];
  state.spawnIndex = 0;
  mocks.client.mockReset();
  mocks.ndJsonStream.mockClear();
  mocks.ndJsonStream.mockImplementation((_output, input) => {
    const reader = input.getReader();
    void reader.read().then(() => reader.cancel());
    return { readable: {}, writable: {} };
  });
  mocks.spawn.mockReset();
  configureAcpMock();
  const runtime = await import("../src/runtime.ts");
  runtime.devinRuntimeTestApi.reset();
  state.deleteCalls = [];
  state.sessions = [];
  state.sessionRequests = [];
  state.children = [];
});

async function waitUntil(predicate: () => boolean): Promise<void> {
  if (predicate()) return;
  await new Promise<void>((resolve) => setImmediate(resolve));
  return waitUntil(predicate);
}

test("selects allow permissions in priority order and handles no options", async () => {
  const { selectPermission } = await import("../src/runtime.ts");

  expect(
    selectPermission({
      sessionId: "session",
      toolCall: { toolCallId: "tool" },
      options: [
        { optionId: "once", name: "Once", kind: "allow_once" },
        { optionId: "always", name: "Always", kind: "allow_always" },
      ],
    }),
  ).toStrictEqual({ outcome: { outcome: "selected", optionId: "always" } });
  expect(
    selectPermission({
      sessionId: "session",
      toolCall: { toolCallId: "tool" },
      options: [{ optionId: "reject", name: "Reject", kind: "reject_once" }],
    }),
  ).toStrictEqual({ outcome: { outcome: "selected", optionId: "reject" } });
  expect(
    selectPermission({ sessionId: "session", toolCall: { toolCallId: "tool" }, options: [] }),
  ).toStrictEqual({ outcome: { outcome: "cancelled" } });
});

test("creates and resolves Devin session ids for pi agent coordination", async () => {
  const { createDevinSessionId, resolveDevinSessionId, runtimeKey } =
    await import("../src/runtime.ts");
  const created = createDevinSessionId();
  expect(created.startsWith("devin-pi:")).toBe(true);
  expect(resolveDevinSessionId("pi-session-a")).toBe("pi-session-a");
  expect(resolveDevinSessionId("").startsWith("devin-pi:")).toBe(true);
  expect(resolveDevinSessionId(undefined).startsWith("devin-pi:")).toBe(true);
  expect(runtimeKey("/tmp/a", "swe-1-7", "pi-a")).toBe("/tmp/a\0swe-1-7\0pi-a");
});

test("continues one Devin ACP session across turns for the same pi session id", async () => {
  installScenario({
    modes: {
      currentModeId: "default",
      availableModes: [
        { id: "default", name: "Default" },
        { id: "bypass", name: "Bypass" },
      ],
    },
    turnUpdates: [[textUpdate("one")], [textUpdate("two")]],
    connectError: undefined,
    setModeError: new Error("unsupported mode"),
    stderr: "",
  });
  const updates: string[] = [];
  const onUpdate: JobInput["onUpdate"] = (update) => {
    if (update.sessionUpdate === "agent_message_chunk" && update.content.type === "text") {
      updates.push(update.content.text);
    }
  };
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");

  await runDevinJob(
    job({
      sessionId: "pi-session-a",
      initialPrompt: "full-transcript-1",
      continuationPrompt: "latest-user-1",
      onUpdate,
    }),
  );
  await runDevinJob(
    job({
      sessionId: "pi-session-a",
      initialPrompt: "full-transcript-2",
      continuationPrompt: "latest-user-2",
      onUpdate,
    }),
  );

  expect(mocks.spawn).toHaveBeenCalledTimes(1);
  expect(devinRuntimeTestApi.pooledCount()).toBe(1);
  expect(state.sessionRequests).toStrictEqual([{ cwd: "/tmp/project-a", mcpServers: [] }]);
  expect(state.sessions).toHaveLength(1);
  expect(state.sessions[0]?.sessionId).toBe("session-1");
  expect(state.sessions[0]?.prompt).toHaveBeenNthCalledWith(1, "full-transcript-1");
  expect(state.sessions[0]?.prompt).toHaveBeenNthCalledWith(2, "latest-user-2");
  expect(state.deleteCalls).toHaveLength(0);
  expect(updates).toStrictEqual(["one", "two"]);
  expect(
    state.permissionHandler?.({
      params: {
        sessionId: "session",
        toolCall: { toolCallId: "tool" },
        options: [{ optionId: "once", name: "Once", kind: "allow_once" }],
      },
    }),
  ).toStrictEqual({ outcome: { outcome: "selected", optionId: "once" } });
});

test("opens a fresh Devin ACP session with the full transcript after compaction", async () => {
  const compacted =
    "USER:\nThe conversation history before this point was compacted into the following summary:\n\n<summary>\nprior work\n</summary>\n\nContinue from the transcript above. Follow the latest user request.";
  installScenario({
    modes: undefined,
    turnUpdates: [[textUpdate("before")], [textUpdate("after")]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { runDevinJob } = await import("../src/runtime.ts");
  await runDevinJob(
    job({
      sessionId: "compact-session",
      initialPrompt: "full-before-compact",
      continuationPrompt: "before-continue",
    }),
  );
  await runDevinJob(
    job({
      sessionId: "compact-session",
      initialPrompt: compacted,
      continuationPrompt: "should-not-use-continuation",
    }),
  );

  expect(mocks.spawn).toHaveBeenCalledTimes(1);
  expect(state.sessionRequests).toHaveLength(2);
  expect(state.sessions).toHaveLength(2);
  expect(state.deleteCalls).toStrictEqual([{ sessionId: "session-1" }]);
  expect(state.sessions[0]?.prompt).toHaveBeenCalledWith("full-before-compact");
  expect(state.sessions[1]?.prompt).toHaveBeenCalledWith(compacted);
});

test("invalidates pooled runtimes for a pi session id", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, invalidateDevinSessionsForPiSession, runDevinJob } =
    await import("../src/runtime.ts");
  await runDevinJob(job({ sessionId: "pi-to-drop", initialPrompt: "hello" }));
  expect(devinRuntimeTestApi.isRunning()).toBe(true);
  invalidateDevinSessionsForPiSession("pi-to-drop");
  expect(devinRuntimeTestApi.isRunning()).toBe(false);
});

test("keeps separate continuing Devin sessions for concurrent subagents A and B", async () => {
  const pendingA: { release: (() => void) | undefined } = { release: undefined };
  const pendingB: { release: (() => void) | undefined } = { release: undefined };
  const blockerA = new Promise<unknown>((resolve) => {
    pendingA.release = () => resolve({ kind: "stop", response: { stopReason: "end_turn" } });
  });
  const blockerB = new Promise<unknown>((resolve) => {
    pendingB.release = () => resolve({ kind: "stop", response: { stopReason: "end_turn" } });
  });
  installScenario({
    modes: undefined,
    turnUpdates: [[blockerA], [textUpdate("a-2")]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  installScenario({
    modes: undefined,
    turnUpdates: [[blockerB], [textUpdate("b-2")]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  const jobA1 = runDevinJob(
    job({
      sessionId: "subagent-a",
      initialPrompt: "a-turn-1",
    }),
  );
  await waitUntil(() => state.sessions.length === 1 && state.children.length === 1);
  const jobB1 = runDevinJob(
    job({
      sessionId: "subagent-b",
      initialPrompt: "b-turn-1",
    }),
  );
  await waitUntil(() => state.sessions.length === 2 && state.children.length === 2);

  const childA = state.children[0];
  const childB = state.children[1];
  expect(mocks.spawn).toHaveBeenCalledTimes(2);
  expect(childA).not.toBe(childB);
  expect(childA?.exitCode).toBe(null);
  expect(childB?.exitCode).toBe(null);
  expect(state.sessions.map((session) => session.sessionId)).toStrictEqual([
    "session-1",
    "session-2",
  ]);
  expect(state.sessions[0]?.prompt).toHaveBeenCalledWith("a-turn-1");
  expect(state.sessions[1]?.prompt).toHaveBeenCalledWith("b-turn-1");
  expect(devinRuntimeTestApi.pooledCount()).toBe(2);

  pendingB.release?.();
  await jobB1;
  expect(childB?.exitCode).toBe(null);
  expect(childA?.exitCode).toBe(null);
  expect(devinRuntimeTestApi.pooledCount()).toBe(2);
  expect(state.deleteCalls).toHaveLength(0);

  pendingA.release?.();
  await jobA1;
  expect(childA?.exitCode).toBe(null);
  expect(childB?.exitCode).toBe(null);
  expect(devinRuntimeTestApi.pooledCount()).toBe(2);

  await runDevinJob(
    job({
      sessionId: "subagent-a",
      initialPrompt: "a-full-2",
      continuationPrompt: "a-turn-2",
    }),
  );
  await runDevinJob(
    job({
      sessionId: "subagent-b",
      initialPrompt: "b-full-2",
      continuationPrompt: "b-turn-2",
    }),
  );

  expect(mocks.spawn).toHaveBeenCalledTimes(2);
  expect(state.sessions).toHaveLength(2);
  expect(state.sessions[0]?.prompt).toHaveBeenNthCalledWith(2, "a-turn-2");
  expect(state.sessions[1]?.prompt).toHaveBeenNthCalledWith(2, "b-turn-2");
  expect(state.deleteCalls).toHaveLength(0);
  expect(devinRuntimeTestApi.pooledCount()).toBe(2);
});

test("serializes concurrent turns on the same pi session id without spawning twice", async () => {
  const pendingFirst: { release: (() => void) | undefined } = { release: undefined };
  const blocker = new Promise<unknown>((resolve) => {
    pendingFirst.release = () => resolve({ kind: "stop", response: { stopReason: "end_turn" } });
  });
  installScenario({
    modes: undefined,
    turnUpdates: [[blocker], [textUpdate("second-turn")]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  const first = runDevinJob(
    job({
      sessionId: "same-session",
      initialPrompt: "turn-1",
    }),
  );
  await waitUntil(() => state.sessions.length === 1);
  const second = runDevinJob(
    job({
      sessionId: "same-session",
      initialPrompt: "unused-2",
      continuationPrompt: "turn-2",
    }),
  );
  await waitUntil(() => state.sessions[0]?.prompt.mock.calls.length === 1);
  expect(mocks.spawn).toHaveBeenCalledTimes(1);
  expect(devinRuntimeTestApi.pooledCount()).toBe(1);

  pendingFirst.release?.();
  await first;
  await second;

  expect(mocks.spawn).toHaveBeenCalledTimes(1);
  expect(state.sessions).toHaveLength(1);
  expect(state.sessions[0]?.prompt).toHaveBeenNthCalledWith(1, "turn-1");
  expect(state.sessions[0]?.prompt).toHaveBeenNthCalledWith(2, "turn-2");
  expect(devinRuntimeTestApi.pooledCount()).toBe(1);
});

test("replaces a dead pooled runtime when the same session key is reused", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { runDevinJob } = await import("../src/runtime.ts");
  await runDevinJob(job({ sessionId: "revive", initialPrompt: "first" }));
  const child = state.children[0];
  const exitHandler = child?.once.mock.calls.find((call) => call[0] === "exit")?.[1];
  if (child) child.exitCode = 0;
  if (typeof exitHandler === "function") exitHandler();
  await runDevinJob(job({ sessionId: "revive", initialPrompt: "second" }));
  expect(mocks.spawn).toHaveBeenCalledTimes(2);
  expect(state.sessions).toHaveLength(2);
});

test("stops the runtime when the child process emits an error", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  await runDevinJob(job({ sessionId: "err", cwd: "/tmp", initialPrompt: "hello" }));
  expect(devinRuntimeTestApi.isRunning()).toBe(true);
  const child = state.children[0];
  const errorHandler = child?.once.mock.calls.find((call) => call[0] === "error")?.[1];
  expect(typeof errorHandler).toBe("function");
  if (typeof errorHandler === "function") errorHandler();
  expect(devinRuntimeTestApi.isRunning()).toBe(false);
});

test("rejects queued jobs when the runtime stops before they run", async () => {
  const pendingFirst: { release: (() => void) | undefined } = { release: undefined };
  const blocker = new Promise<unknown>((resolve) => {
    pendingFirst.release = () => resolve({ kind: "stop", response: { stopReason: "end_turn" } });
  });
  installScenario({
    modes: undefined,
    turnUpdates: [[blocker]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  const first = runDevinJob(job({ sessionId: "queue-stop", initialPrompt: "first" }));
  await waitUntil(() => state.sessions.length === 1);
  const second = runDevinJob(job({ sessionId: "queue-stop", initialPrompt: "second" }));
  await waitUntil(() => state.sessions[0]?.prompt.mock.calls.length === 1);
  devinRuntimeTestApi.stop();
  await expect(second).rejects.toThrow("Devin ACP runtime is stopped");
  pendingFirst.release?.();
  await first.catch(() => undefined);
});

test("pools separate processes for different cwd or model keys", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");

  await runDevinJob(
    job({
      sessionId: "shared",
      modelId: "swe-1-7",
      initialPrompt: "first",
    }),
  );
  await runDevinJob(
    job({
      sessionId: "shared",
      modelId: "swe-1-7-medium",
      initialPrompt: "second",
    }),
  );

  expect(mocks.spawn).toHaveBeenCalledTimes(2);
  expect(devinRuntimeTestApi.pooledCount()).toBe(2);
});

test("reports ACP failures with captured stderr and stops the runtime", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: new Error("connection closed"),
    setModeError: undefined,
    stderr: "authentication required",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");

  await expect(
    runDevinJob(job({ sessionId: "s1", cwd: "/tmp", initialPrompt: "hello" })),
  ).rejects.toThrow("connection closed: authentication required");
  expect(devinRuntimeTestApi.isRunning()).toBe(false);
});

test("cancels an active session when its signal aborts without keeping the job pending", async () => {
  const controller = new AbortController();
  const pending: { reject: ((error: Error) => void) | undefined } = { reject: undefined };
  const update = new Promise<never>((_resolve, reject) => {
    pending.reject = reject;
  });
  installScenario({
    modes: undefined,
    turnUpdates: [[update]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { runDevinJob } = await import("../src/runtime.ts");
  const running = runDevinJob(
    job({
      sessionId: "s1",
      cwd: "/tmp",
      initialPrompt: "hello",
      signal: controller.signal,
    }),
  );
  await new Promise<void>((resolve) => setImmediate(resolve));
  controller.abort();
  pending.reject?.(new Error("cancelled"));
  await expect(running).rejects.toThrow("cancelled");
  expect(state.sessions[0]?.sessionId).toBe("session-1");
});

test("stops an idle runtime after the configured TTL", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  devinRuntimeTestApi.setIdleTtlMs(20);
  await runDevinJob(job({ sessionId: "s1", cwd: "/tmp", initialPrompt: "hello" }));
  expect(devinRuntimeTestApi.isRunning()).toBe(true);
  await new Promise<void>((resolve) => setTimeout(resolve, 50));
  expect(devinRuntimeTestApi.isRunning()).toBe(false);
  expect(state.deleteCalls).toStrictEqual([{ sessionId: "session-1" }]);
});

test("rejects already-aborted jobs before prompting Devin", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[textUpdate("should-not-run")]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const controller = new AbortController();
  controller.abort(new Error("already aborted"));
  const { runDevinJob } = await import("../src/runtime.ts");
  await expect(
    runDevinJob(
      job({
        sessionId: "s1",
        cwd: "/tmp",
        initialPrompt: "hello",
        signal: controller.signal,
      }),
    ),
  ).rejects.toThrow("already aborted");
  expect(state.sessions).toHaveLength(0);
});

test("rejects aborted jobs that use a non-Error abort reason", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const controller = new AbortController();
  controller.abort("stopped");
  const { runDevinJob } = await import("../src/runtime.ts");
  await expect(
    runDevinJob(
      job({
        sessionId: "s1",
        cwd: "/tmp",
        initialPrompt: "hello",
        signal: controller.signal,
      }),
    ),
  ).rejects.toThrow("Devin ACP request aborted");
});

test("stops immediately after a job when --print is passed", async () => {
  const previous: string[] = [...process.argv];
  process.argv = [...previous, "--print"];
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  try {
    await runDevinJob(job({ sessionId: "s1", cwd: "/tmp", initialPrompt: "hello" }));
    expect(devinRuntimeTestApi.isRunning()).toBe(false);
  } finally {
    process.argv = previous;
  }
});

test("selects allow_once when allow_always is absent", async () => {
  const { selectPermission } = await import("../src/runtime.ts");
  expect(
    selectPermission({
      sessionId: "session",
      toolCall: { toolCallId: "tool" },
      options: [
        { optionId: "reject", name: "Reject", kind: "reject_once" },
        { optionId: "once", name: "Once", kind: "allow_once" },
      ],
    }),
  ).toStrictEqual({ outcome: { outcome: "selected", optionId: "once" } });
});

test("exposes stop helpers for the shared runtime", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  await runDevinJob(job({ sessionId: "s1", cwd: "/tmp", initialPrompt: "hello" }));
  expect(devinRuntimeTestApi.isRunning()).toBe(true);
  devinRuntimeTestApi.stop();
  expect(devinRuntimeTestApi.isRunning()).toBe(false);
  devinRuntimeTestApi.stop();
});

test("stops the shared runtime when the process is about to exit", async () => {
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  await runDevinJob(job({ sessionId: "s1", cwd: "/tmp", initialPrompt: "hello" }));
  expect(devinRuntimeTestApi.isRunning()).toBe(true);
  process.emit("beforeExit", 0);
  expect(devinRuntimeTestApi.isRunning()).toBe(false);
});

test("stops immediately after a job in print mode", async () => {
  const previous: string[] = [...process.argv];
  process.argv = [...previous, "-p"];
  installScenario({
    modes: undefined,
    turnUpdates: [[]],
    connectError: undefined,
    setModeError: undefined,
    stderr: "",
  });
  const { devinRuntimeTestApi, runDevinJob } = await import("../src/runtime.ts");
  try {
    await runDevinJob(job({ sessionId: "s1", cwd: "/tmp", initialPrompt: "hello" }));
    expect(devinRuntimeTestApi.isRunning()).toBe(false);
  } finally {
    process.argv = previous;
  }
});
