// This TypeScript file is executed with Bun.
import { createHash } from "node:crypto";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import cfAffinity, { affinityFor, HEADER, PI_DEFAULT, sessionKey } from "./index.ts";

type Handler = (event: { headers: Record<string, string | null> }, ctx: unknown) => unknown;

function harness(
  provider: string | undefined,
  sessionId = "s1",
  file?: string,
): {
  readonly headers: Record<string, string | null>;
  readonly emit: () => void;
} {
  const handlers = new Map<string, Handler>();
  cfAffinity({
    on: (name: string, handler: Handler) => {
      handlers.set(name, handler);
    },
  } as unknown as ExtensionAPI);
  const headers: Record<string, string | null> = {};
  return {
    headers,
    emit: () => {
      handlers.get("before_provider_headers")?.(
        { headers },
        {
          model: provider === undefined ? undefined : { provider },
          sessionManager: {
            getSessionFile: () => file,
            getSessionId: () => sessionId,
          },
        },
      );
    },
  };
}

test("a gateway request is pinned to this session's Workers AI instance", () => {
  const box = harness("cloudflare-ai-gateway");
  box.emit();
  expect(box.headers[HEADER]).toBe("pi-s1");
});

test("Workers AI without the gateway is pinned the same way", () => {
  const box = harness("cloudflare-workers-ai");
  box.emit();
  expect(box.headers[HEADER]).toBe("pi-s1");
});

test("other providers are left alone", () => {
  const box = harness("anthropic");
  box.emit();
  expect(box.headers).toStrictEqual({});

  const withoutModel = harness(undefined);
  withoutModel.emit();
  expect(withoutModel.headers).toStrictEqual({});
});

test("a value another handler chose wins", () => {
  const box = harness("cloudflare-ai-gateway");
  box.headers[HEADER] = "pi-other";
  box.emit();
  expect(box.headers[HEADER]).toBe("pi-other");
});

test("Pi's own constant is replaced, because it pins every session to one instance", () => {
  const box = harness("cloudflare-ai-gateway");
  box.headers[HEADER] = PI_DEFAULT;
  box.emit();
  expect(box.headers[HEADER]).toBe("pi-s1");
});

test("a headless run is pinned by its session file, not by the shared placeholder", () => {
  const first = harness("cloudflare-ai-gateway", "agent", "/tmp/a.jsonl");
  const second = harness("cloudflare-ai-gateway", "agent", "/tmp/b.jsonl");
  first.emit();
  second.emit();
  expect(first.headers[HEADER]).toMatch(/^pi-[\da-f]{12}$/);
  expect(second.headers[HEADER]).toMatch(/^pi-[\da-f]{12}$/);
  expect(first.headers[HEADER]).not.toBe(second.headers[HEADER]);
});

test("a headless run without a session file still gets its own value", () => {
  const box = harness("cloudflare-ai-gateway", "agent");
  box.emit();
  expect(box.headers[HEADER]).toBe(`pi-pid-${String(process.pid)}`);
});

test("only the Cloudflare providers map to an affinity", () => {
  expect(affinityFor("cloudflare-ai-gateway", "s1")).toBe("pi-s1");
  expect(affinityFor("cloudflare-workers-ai", "s2")).toBe("pi-s2");
  expect(affinityFor("openai", "s1")).toBeUndefined();
  expect(affinityFor(undefined, "s1")).toBeUndefined();
});

test("a real session id is used as it is, and an empty one falls back", () => {
  expect(sessionKey("01a1eefb", "/tmp/a.jsonl")).toBe("01a1eefb");
  expect(sessionKey("", "/tmp/a.jsonl")).toBe(
    createHash("sha1").update("/tmp/a.jsonl").digest("hex").slice(0, 12),
  );
  expect(sessionKey("", undefined)).toBe(`pid-${String(process.pid)}`);
});
