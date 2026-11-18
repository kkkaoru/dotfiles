// This TypeScript file is executed with Bun.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import cfAffinity, { affinityFor, HEADER } from "./index.ts";

type Handler = (event: { headers: Record<string, string | null> }, ctx: unknown) => unknown;

function harness(provider: string | undefined): {
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
          sessionManager: { getSessionId: () => "s1" },
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

test("a header that is already set wins", () => {
  const box = harness("cloudflare-ai-gateway");
  box.headers[HEADER] = "mine";
  box.emit();
  expect(box.headers[HEADER]).toBe("mine");
});

test("only the Cloudflare providers map to an affinity", () => {
  expect(affinityFor("cloudflare-ai-gateway", "s1")).toBe("pi-s1");
  expect(affinityFor("cloudflare-workers-ai", "s2")).toBe("pi-s2");
  expect(affinityFor("openai", "s1")).toBeUndefined();
  expect(affinityFor(undefined, "s1")).toBeUndefined();
});
