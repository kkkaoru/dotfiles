// This file runs with bun.
import { createServer, get as httpGet } from "node:http";
import { URL } from "node:url";
import { afterEach, expect, it, vi } from "vitest";

import {
  authorizeUrl,
  credentialFromTokenResponse,
  emailFromIdToken,
  exchangeCode,
  login,
  pkcePair,
  refreshCredential,
} from "../src/oauth.ts";
import type { WireResponse } from "../src/transport.ts";

const ORIGINAL_ID = process.env["MY_ANTIGRAVITY_CLIENT_ID"];
const ORIGINAL_SECRET = process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];

afterEach(() => {
  if (ORIGINAL_ID === undefined) {
    delete process.env["MY_ANTIGRAVITY_CLIENT_ID"];
  } else {
    process.env["MY_ANTIGRAVITY_CLIENT_ID"] = ORIGINAL_ID;
  }
  if (ORIGINAL_SECRET === undefined) {
    delete process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];
  } else {
    process.env["MY_ANTIGRAVITY_CLIENT_SECRET"] = ORIGINAL_SECRET;
  }
});

function fakeJwt(payload: Record<string, unknown>): string {
  const header = Buffer.from('{"alg":"none"}').toString("base64url");
  const body = Buffer.from(JSON.stringify(payload)).toString("base64url");
  return `${header}.${body}.sig`;
}

const jsonWire = (status: number, body: unknown): WireResponse => ({
  status,
  headers: {},
  body: (async function* () {})(),
  text: async () => (typeof body === "string" ? body : JSON.stringify(body)),
});

function getStatus(url: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const req = httpGet(url, (res) => {
      res.resume();
      resolve(res.statusCode ?? 0);
    });
    req.on("error", reject);
  });
}

it("generates PKCE pairs and authorization URLs", () => {
  process.env["MY_ANTIGRAVITY_CLIENT_ID"] = "test-client-id";
  process.env["MY_ANTIGRAVITY_CLIENT_SECRET"] = "test-client-secret";
  const first = pkcePair();
  const second = pkcePair();
  expect(first.verifier).not.toBe(second.verifier);
  expect(first.challenge).not.toBe(first.verifier);
  expect(first.challenge).toMatch(/^[A-Za-z0-9_-]+$/);

  const url = new URL(authorizeUrl("http://127.0.0.1:51123/oauth-callback", "chal", "st"));
  expect(url.searchParams.get("client_id")).toBe("test-client-id");
  expect(url.searchParams.get("code_challenge")).toBe("chal");
  expect(url.searchParams.get("code_challenge_method")).toBe("S256");
  expect(url.searchParams.get("state")).toBe("st");
  expect(url.searchParams.get("access_type")).toBe("offline");
});

it("decodes email claims and builds credentials from token responses", () => {
  const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
  expect(emailFromIdToken(undefined)).toBeUndefined();
  expect(emailFromIdToken("bad")).toBeUndefined();
  expect(emailFromIdToken("a.not-json.c")).toBeUndefined();
  expect(warnSpy).toHaveBeenCalledTimes(1);
  expect(emailFromIdToken(fakeJwt({ email: "user@example.com" }))).toBe("user@example.com");
  expect(emailFromIdToken(fakeJwt({ email: 123 }))).toBeUndefined();

  const cred = credentialFromTokenResponse(
    { access_token: "acc", expires_in: 120, id_token: fakeJwt({ email: "a@b.c" }) },
    { refresh: "ref", projectId: "proj", email: "old@b.c" },
  );
  expect(cred).toMatchObject({
    type: "oauth",
    access: "acc",
    refresh: "ref",
    projectId: "proj",
    email: "a@b.c",
  });
  expect(cred.expires).toBeGreaterThan(Date.now());

  expect(() => credentialFromTokenResponse({ error: "access_denied" })).toThrow(/access_denied/);
  expect(() => credentialFromTokenResponse({ error: "x", error_description: "denied" })).toThrow(
    /denied/,
  );
  expect(() => credentialFromTokenResponse({})).toThrow(/no access token/);
});

it("exchanges codes and refreshes tokens cleanly", async () => {
  process.env["MY_ANTIGRAVITY_CLIENT_ID"] = "test-client-id";
  process.env["MY_ANTIGRAVITY_CLIENT_SECRET"] = "test-client-secret";

  const exchanged = await exchangeCode({
    code: "code-1",
    redirectUri: "http://127.0.0.1:1/oauth-callback",
    verifier: "ver",
    post: async () =>
      jsonWire(200, {
        access_token: "new-acc",
        refresh_token: "new-ref",
        expires_in: 60,
        id_token: fakeJwt({ email: "exchanged@example.com" }),
      }),
  });
  expect(exchanged.access).toBe("new-acc");
  expect(exchanged.refresh).toBe("new-ref");
  expect(exchanged.email).toBe("exchanged@example.com");

  await expect(
    exchangeCode({
      code: "code-2",
      redirectUri: "http://127.0.0.1:1/oauth-callback",
      verifier: "ver",
      post: async () => jsonWire(400, { error: "bad_code", error_description: "expired" }),
    }),
  ).rejects.toThrow(/expired/);

  await expect(
    exchangeCode({
      code: "code-3",
      redirectUri: "http://127.0.0.1:1/oauth-callback",
      verifier: "ver",
      post: async () => jsonWire(200, "null"),
    }),
  ).rejects.toThrow(/no access token/);

  const refreshed = await refreshCredential(
    {
      type: "oauth",
      access: "old",
      refresh: "ref-1",
      expires: 0,
      projectId: "p1",
      email: "user@example.com",
    },
    {
      post: async () => jsonWire(200, { access_token: "new-acc", refresh_token: "new-ref" }),
    },
  );
  expect(refreshed.access).toBe("new-acc");
  expect(refreshed.projectId).toBe("p1");
  expect(refreshed.email).toBe("user@example.com");

  const keptRefresh = await refreshCredential(
    { type: "oauth", access: "old", refresh: "ref-keep", expires: 0 },
    { post: async () => jsonWire(200, { access_token: "acc-2" }) },
  );
  expect(keptRefresh.refresh).toBe("ref-keep");
  expect(keptRefresh.expires).toBeGreaterThan(Date.now());

  await expect(
    refreshCredential({ type: "oauth", access: "a", refresh: "", expires: 0 }),
  ).rejects.toThrow(/no refresh token/);
});

it("runs loopback login and rejects invalid callbacks or busy ports", async () => {
  process.env["MY_ANTIGRAVITY_CLIENT_ID"] = "test-client-id";
  process.env["MY_ANTIGRAVITY_CLIENT_SECRET"] = "test-client-secret";
  const postOk = async () =>
    jsonWire(200, { access_token: "tok", refresh_token: "ref", expires_in: 300 });

  const cred = await login(
    async (authUrl) => {
      const parsed = new URL(authUrl);
      const redirect = new URL(parsed.searchParams.get("redirect_uri")!);
      const state = parsed.searchParams.get("state")!;
      expect(await getStatus(`${redirect.origin}/other`)).toBe(404);
      expect(
        await getStatus(`${redirect.origin}${redirect.pathname}?code=abc&state=${state}`),
      ).toBe(200);
    },
    0,
    postOk,
  );
  expect(cred.access).toBe("tok");

  await expect(
    login(
      async (authUrl) => {
        const parsed = new URL(authUrl);
        const redirect = new URL(parsed.searchParams.get("redirect_uri")!);
        await getStatus(`${redirect.origin}${redirect.pathname}?state=bad`);
      },
      0,
      postOk,
    ),
  ).rejects.toThrow(/returned no code/);

  await expect(
    login(
      async (authUrl) => {
        const parsed = new URL(authUrl);
        const redirect = new URL(parsed.searchParams.get("redirect_uri")!);
        await getStatus(`${redirect.origin}${redirect.pathname}?error=access_denied&state=bad`);
      },
      0,
      postOk,
    ),
  ).rejects.toThrow(/access_denied/);

  const blocker = createServer();
  await new Promise<void>((resolve) => blocker.listen(0, "127.0.0.1", () => resolve()));
  const address = blocker.address();
  const busyPort = address && typeof address === "object" ? address.port : 0;
  try {
    await expect(login(() => {}, busyPort, postOk)).rejects.toThrow();
  } finally {
    blocker.close();
  }
});
