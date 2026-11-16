// This file runs with bun.
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

const ORIGINAL_FETCH = globalThis.fetch;
const ORIGINAL_ID = process.env["MY_ANTIGRAVITY_CLIENT_ID"];
const ORIGINAL_SECRET = process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];

afterEach(() => {
  globalThis.fetch = ORIGINAL_FETCH;
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

function requestUrl(input: string | URL | Request): string {
  if (typeof input === "string") {
    return input;
  }
  return input instanceof URL ? input.href : input.url;
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

it("exchanges codes, refreshes tokens, and runs loopback login", async () => {
  process.env["MY_ANTIGRAVITY_CLIENT_ID"] = "test-client-id";
  process.env["MY_ANTIGRAVITY_CLIENT_SECRET"] = "test-client-secret";
  globalThis.fetch = (async () =>
    new Response(
      JSON.stringify({ access_token: "new-acc", refresh_token: "new-ref", expires_in: 60 }),
      { status: 200 },
    )) as unknown as typeof fetch;

  const exchanged = await exchangeCode("code-1", "http://127.0.0.1:1/oauth-callback", "ver");
  expect(exchanged.access).toBe("new-acc");
  expect(exchanged.refresh).toBe("new-ref");

  const refreshed = await refreshCredential({
    type: "oauth",
    access: "old",
    refresh: "ref-1",
    expires: 0,
    projectId: "p1",
    email: "user@example.com",
  });
  expect(refreshed.access).toBe("new-acc");
  expect(refreshed.projectId).toBe("p1");
  expect(refreshed.email).toBe("user@example.com");

  await expect(
    refreshCredential({ type: "oauth", access: "a", refresh: "", expires: 0 }),
  ).rejects.toThrow(/no refresh token/);

  globalThis.fetch = (async (input: string | URL | Request) => {
    const urlText = requestUrl(input);
    if (urlText.includes("oauth2.googleapis.com")) {
      return new Response(
        JSON.stringify({ access_token: "tok", refresh_token: "ref", expires_in: 300 }),
        { status: 200 },
      );
    }
    return ORIGINAL_FETCH(input);
  }) as unknown as typeof fetch;

  const cred = await login(async (authUrl) => {
    const parsed = new URL(authUrl);
    const redirect = new URL(parsed.searchParams.get("redirect_uri")!);
    const state = parsed.searchParams.get("state")!;
    const wrong = await ORIGINAL_FETCH(`${redirect.origin}/other`);
    expect(wrong.status).toBe(404);
    const ok = await ORIGINAL_FETCH(
      `${redirect.origin}${redirect.pathname}?code=abc&state=${state}`,
    );
    expect(ok.status).toBe(200);
  }, 0);
  expect(cred.access).toBe("tok");

  await expect(
    login(async (authUrl) => {
      const parsed = new URL(authUrl);
      const redirect = new URL(parsed.searchParams.get("redirect_uri")!);
      await ORIGINAL_FETCH(`${redirect.origin}${redirect.pathname}?error=access_denied&state=bad`);
    }, 0),
  ).rejects.toThrow(/access_denied/);
});
