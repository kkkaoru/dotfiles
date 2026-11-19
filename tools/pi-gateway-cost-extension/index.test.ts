// This TypeScript file is executed with Bun.
import { spawnSync } from "node:child_process";
import { homedir } from "node:os";
import path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { beforeEach, expect, test, vi } from "vitest";
import gatewayCost, { secretsScript } from "./index.ts";

const files = new Map<string, string>();
const agentDir = { value: "/agent" };
const aged = { status: 0, stdout: '{"accountId":"secret-acct"}' };

vi.mock("node:fs", () => ({
  existsSync: (file: string) => files.has(file),
  readFileSync: (file: string) => files.get(file),
}));
vi.mock("node:child_process", () => ({ spawnSync: vi.fn(() => aged) }));
vi.mock("@earendil-works/pi-coding-agent", () => ({ getAgentDir: () => agentDir.value }));

const SETTINGS = JSON.stringify({ "pi-gateway-cost": { gateway: "pi-agent" } });
const WRANGLER = path.join(homedir(), ".wrangler", "config", "default.toml");
const SCRIPT = secretsScript();

type Handler = (event: unknown, ctx: unknown) => unknown;

function harness(): {
  readonly notes: string[];
  readonly statuses: (string | undefined)[];
  readonly run: (event: string) => Promise<void>;
} {
  const notes: string[] = [];
  const statuses: (string | undefined)[] = [];
  const handlers = new Map<string, Handler>();
  gatewayCost({
    on: (name: string, handler: Handler) => {
      handlers.set(name, handler);
    },
  } as unknown as ExtensionAPI);
  return {
    notes,
    statuses,
    run: async (event) => {
      await handlers.get(event)?.(
        {},
        {
          ui: {
            notify: (note: string) => notes.push(note),
            setStatus: (_key: string, text: string | undefined) => statuses.push(text),
          },
        },
      );
    },
  };
}

const answer = (cost: number): Response =>
  ({
    json: async () => ({
      data: { viewer: { accounts: [{ aiGatewayRequestsAdaptiveGroups: [{ sum: { cost } }] }] } },
    }),
    ok: true,
  }) as Response;

function authHeader(index = 0): string | undefined {
  const call = vi.mocked(globalThis.fetch).mock.calls[index];
  const headers = (call?.[1]?.headers ?? {}) as Record<string, string>;
  return headers.authorization;
}

beforeEach(() => {
  files.clear();
  files.set("/agent/settings.json", SETTINGS);
  files.set(WRANGLER, 'oauth_token = "wrangler-token"\n');
  files.set(SCRIPT, "#!/bin/bash");
  vi.stubEnv("CLOUDFLARE_API_TOKEN", "");
  vi.stubEnv("CLOUDFLARE_ACCOUNT_ID", "env-acct");
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => answer(16.7128)),
  );
});

test("the gateway's own cost appears in the status", async () => {
  const { statuses, run } = harness();
  await run("session_start");
  await vi.waitFor(() => {
    expect(statuses).toStrictEqual(["gateway $16.7/24h"]);
  });
  expect(authHeader()).toBe("Bearer wrangler-token");
});

test("an environment token is used when it is set", async () => {
  vi.stubEnv("CLOUDFLARE_API_TOKEN", "env-token");
  const { statuses, run } = harness();
  await run("session_start");
  await vi.waitFor(() => {
    expect(statuses).toHaveLength(1);
  });
  expect(authHeader()).toBe("Bearer env-token");
});

test("a read that fails warns once and shows nothing", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({ ok: false }) as Response),
  );
  const { notes, statuses, run } = harness();
  await run("session_start");
  await vi.waitFor(() => {
    expect(notes).toHaveLength(1);
  });
  expect(statuses).toStrictEqual([]);
  await run("turn_end");
  expect(notes).toHaveLength(1);
});

test("a throwing fetch is swallowed", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => {
      throw new Error("offline");
    }),
  );
  const { notes, statuses, run } = harness();
  await run("session_start");
  await vi.waitFor(() => {
    expect(globalThis.fetch).toHaveBeenCalledTimes(1);
  });
  expect(notes).toStrictEqual([]);
  expect(statuses).toStrictEqual([]);
});

test("a 401 renews wrangler's token and asks again", async () => {
  files.set(WRANGLER, 'oauth_token = "stale-token"\n');
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockImplementationOnce(async () => {
        files.set(WRANGLER, 'oauth_token = "renewed-token"\n');
        return { ok: false, status: 401 } as Response;
      })
      .mockResolvedValue(answer(2.5)),
  );
  const { statuses, run } = harness();
  await run("session_start");
  await vi.waitFor(() => {
    expect(statuses).toStrictEqual(["gateway $2.5/24h"]);
  });
  expect(vi.mocked(spawnSync).mock.calls[1]?.[1]).toStrictEqual(["whoami"]);
  expect(authHeader(1)).toBe("Bearer renewed-token");
});

test("a 401 with nothing to renew to warns once", async () => {
  files.set(WRANGLER, 'oauth_token = "stale-token"\n');
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => {
      files.delete(WRANGLER);
      return { ok: false, status: 401 } as Response;
    }),
  );
  const { notes, statuses, run } = harness();
  await run("session_start");
  await vi.waitFor(() => {
    expect(notes).toHaveLength(1);
  });
  expect(statuses).toStrictEqual([]);
  expect(globalThis.fetch).toHaveBeenCalledTimes(2);
});

test("a machine without the section or a token asks nothing", async () => {
  files.set("/agent/settings.json", JSON.stringify({}));
  await harness().run("session_start");
  expect(globalThis.fetch).not.toHaveBeenCalled();

  files.delete("/agent/settings.json");
  await harness().run("session_start");
  expect(globalThis.fetch).not.toHaveBeenCalled();

  files.set("/agent/settings.json", SETTINGS);
  files.delete(WRANGLER);
  await harness().run("session_start");
  expect(globalThis.fetch).not.toHaveBeenCalled();

  files.set(WRANGLER, 'oauth_token = "wrangler-token"\n');
  vi.stubEnv("CLOUDFLARE_ACCOUNT_ID", "");
  files.delete(SCRIPT);
  await harness().run("session_start");
  expect(globalThis.fetch).not.toHaveBeenCalled();
});

test("the account id comes from the encrypted secret when the environment has none", async () => {
  vi.stubEnv("CLOUDFLARE_ACCOUNT_ID", "");
  const { statuses, run } = harness();
  await run("session_start");
  expect(vi.mocked(spawnSync).mock.calls[0]?.[1]).toStrictEqual([
    SCRIPT,
    "decrypt",
    "gateway-cost",
  ]);
  await vi.waitFor(() => {
    expect(statuses).toStrictEqual(["gateway $16.7/24h"]);
  });
  const [call] = vi.mocked(globalThis.fetch).mock.calls;
  expect(JSON.stringify(call?.[1]?.body)).toContain("secret-acct");
});

test("a secret the script cannot decrypt is treated as absent", async () => {
  vi.stubEnv("CLOUDFLARE_ACCOUNT_ID", "");
  aged.status = 2;
  const { statuses, run } = harness();
  await run("session_start");
  await Promise.resolve();
  expect(statuses).toStrictEqual([]);
  expect(globalThis.fetch).not.toHaveBeenCalled();
  aged.status = 0;
});

test("an answer without a cost warns once", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(
      async () =>
        ({ json: async () => ({ data: { viewer: { accounts: [] } } }), ok: true }) as Response,
    ),
  );
  const { notes, statuses, run } = harness();
  await run("session_start");
  await vi.waitFor(() => {
    expect(notes).toHaveLength(1);
  });
  expect(statuses).toStrictEqual([]);
});

test("a later failed read does not warn again", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({ ok: false }) as Response),
  );
  const { notes, run } = harness();
  const clock = vi.spyOn(Date, "now");
  clock.mockReturnValueOnce(1_000_000);
  await run("session_start");
  await vi.waitFor(() => {
    expect(notes).toHaveLength(1);
  });
  clock.mockReturnValueOnce(1_000_000 + 120_000);
  await run("turn_end");
  await vi.waitFor(() => {
    expect(globalThis.fetch).toHaveBeenCalledTimes(2);
  });
  expect(notes).toHaveLength(1);
});

test("a second turn within the minute does not ask again", async () => {
  const { statuses, run } = harness();
  await run("session_start");
  await vi.waitFor(() => {
    expect(statuses).toHaveLength(1);
  });
  await run("turn_end");
  await Promise.resolve();
  expect(globalThis.fetch).toHaveBeenCalledTimes(1);
});
