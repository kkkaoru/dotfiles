// Runs with Bun; session entries are supplied in memory, not read from agmsg storage.
import { expect, it, vi } from "vitest";
import executorToolRouting, { type ToolRoutingHost } from "./pi.ts";

type Handler = Parameters<ToolRoutingHost["on"]>[1];

it("disables only the direct model tool at session start without disabling the extension", () => {
  const handlers = new Map<string, Handler>();
  const setActiveTools = vi.fn<ToolRoutingHost["setActiveTools"]>();
  const on = vi.fn<ToolRoutingHost["on"]>((event, handler) => {
    handlers.set(event, handler);
  });
  executorToolRouting({
    on,
    getActiveTools: () => ["bash", "read", "agmsg", "tmux_exec"],
    setActiveTools,
  });
  expect(on).toHaveBeenCalledWith("session_start", expect.any(Function));
  expect(setActiveTools).not.toHaveBeenCalled();
  handlers.get("session_start")?.({});
  expect(setActiveTools).toHaveBeenCalledExactlyOnceWith([
    "bash",
    "read",
    "tmux_exec",
  ]);
});

it("preserves all tools when the direct agmsg tool is absent", () => {
  const setActiveTools = vi.fn<ToolRoutingHost["setActiveTools"]>();
  executorToolRouting({
    on: (_event, handler) => {
      handler({});
    },
    getActiveTools: () => ["bash", "read"],
    setActiveTools,
  });
  expect(setActiveTools).toHaveBeenCalledExactlyOnceWith(["bash", "read"]);
});

it("passes only the latest selected session identity and originating project into the prompt", () => {
  const handlers = new Map<string, Handler>();
  executorToolRouting({
    on: (event, handler) => {
      handlers.set(event, handler);
    },
    getActiveTools: () => [],
    setActiveTools: vi.fn(),
  });
  const entries: unknown[] = [
    {
      type: "custom",
      customType: "agmsg-active-identity",
      data: { identity: { agent: "old", teams: ["team"] } },
    },
    {
      type: "custom",
      customType: "agmsg-active-identity",
      data: {
        state: "selected",
        identity: { agent: "alice", teams: ["team"] },
      },
    },
    {
      type: "custom",
      customType: "agmsg-active-identity",
      data: { identity: { agent: 12, teams: ["team"] } },
    },
    {
      type: "custom",
      customType: "unrelated",
      data: { private: "not included" },
    },
  ];
  expect(
    handlers.get("before_agent_start")?.(
      { systemPrompt: "Base prompt" },
      { cwd: "/project", sessionManager: { getEntries: () => entries } },
    ),
  ).toStrictEqual({
    systemPrompt:
      'Base prompt\n\nFor agmsg via Executor, use this Pi session\'s selected identity (not another registered name): {"project":"/project","agent":"alice","teams":["team"]}. Verify it with agmsg_whoami; do not change automatic inbox delivery.',
  });
});

it.each([{ state: "cleared" }, { identity: null }])(
  "does not resurrect a cleared identity: $state $identity",
  (data) => {
    const handlers = new Map<string, Handler>();
    executorToolRouting({
      on: (event, handler) => {
        handlers.set(event, handler);
      },
      getActiveTools: () => [],
      setActiveTools: vi.fn(),
    });
    const entries = [
      {
        type: "custom",
        customType: "agmsg-active-identity",
        data: { identity: { agent: "old", teams: ["team"] } },
      },
      { type: "custom", customType: "agmsg-active-identity", data },
    ];
    expect(
      handlers.get("before_agent_start")?.(
        { systemPrompt: "Base" },
        { cwd: "/project", sessionManager: { getEntries: () => entries } },
      ),
    ).toBeUndefined();
  },
);

it.each([null, "invalid", {}, { systemPrompt: 12 }, { systemPrompt: "Base" }])(
  "handles missing context, malformed events and sessions without identities",
  (event) => {
    const handlers = new Map<string, Handler>();
    executorToolRouting({
      on: (name, handler) => {
        handlers.set(name, handler);
      },
      getActiveTools: () => [],
      setActiveTools: vi.fn(),
    });
    expect(handlers.get("before_agent_start")?.(event)).toBeUndefined();
    expect(
      handlers.get("before_agent_start")?.(event, {
        cwd: "/project",
        sessionManager: { getEntries: () => [] },
      }),
    ).toBeUndefined();
  },
);
