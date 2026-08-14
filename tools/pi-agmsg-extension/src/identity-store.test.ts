import { describe, expect, it, vi } from "vitest";
import type { RuntimeContext } from "./contracts.ts";
import { SessionIdentityStore } from "./identity-store.ts";

function contextWithEntries(entries: readonly unknown[]): RuntimeContext {
  return {
    cwd: "/project",
    hasUI: true,
    sessionManager: { getBranch: (): readonly unknown[] => entries },
    signal: undefined,
    ui: {
      confirm: vi.fn(async () => true),
      editor: vi.fn(),
      notify: vi.fn(),
      select: vi.fn(),
      setStatus: vi.fn(),
    },
  };
}

describe("SessionIdentityStore", () => {
  it("restores the latest valid identity from the pi session branch", () => {
    const appendEntry = vi.fn();
    const store = new SessionIdentityStore({ appendEntry });
    const context: RuntimeContext = contextWithEntries([
      { customType: "agmsg-active-identity", data: { identity: null }, type: "custom" },
      {
        customType: "agmsg-active-identity",
        data: { identity: { agent: "alice", teams: ["one", "two"] } },
        type: "custom",
      },
    ]);
    expect(store.load(context)).toStrictEqual({ agent: "alice", teams: ["one", "two"] });
    store.save({ agent: "alice", teams: ["one", "two"] });
    expect(appendEntry).not.toHaveBeenCalled();
  });

  it("persists identity changes and explicit clearing", () => {
    const appendEntry = vi.fn();
    const store = new SessionIdentityStore({ appendEntry });
    store.load(contextWithEntries([]));
    store.save({ agent: "bob", teams: ["one"] });
    store.save(undefined);
    expect(appendEntry.mock.calls).toStrictEqual([
      ["agmsg-active-identity", { identity: { agent: "bob", teams: ["one"] } }],
      ["agmsg-active-identity", { identity: null }],
    ]);
  });

  it("ignores malformed session entries", () => {
    const store = new SessionIdentityStore({ appendEntry: vi.fn() });
    const context: RuntimeContext = contextWithEntries([
      null,
      { type: "message" },
      {
        customType: "agmsg-active-identity",
        data: { identity: { agent: 7, teams: [] } },
        type: "custom",
      },
      {
        customType: "agmsg-active-identity",
        data: { identity: { agent: "alice" } },
        type: "custom",
      },
      { customType: "other", data: { identity: null }, type: "custom" },
    ]);
    expect(store.load(context)).toBeUndefined();
  });
});
