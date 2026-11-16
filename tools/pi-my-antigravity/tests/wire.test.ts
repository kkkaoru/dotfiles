// This file runs with bun.
import { afterEach, expect, it } from "vitest";

import { cliVersion, oauthClientConfig, userAgent } from "../src/constants.ts";
import { TRANSPORT_TLS_OPTIONS, wireHeaders } from "../src/transport.ts";

const ORIGINAL_UA = process.env["MY_ANTIGRAVITY_USER_AGENT"];
const ORIGINAL_ID = process.env["MY_ANTIGRAVITY_CLIENT_ID"];
const ORIGINAL_SECRET = process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];

afterEach(() => {
  if (ORIGINAL_UA === undefined) {
    delete process.env["MY_ANTIGRAVITY_USER_AGENT"];
  } else {
    process.env["MY_ANTIGRAVITY_USER_AGENT"] = ORIGINAL_UA;
  }
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

it("reads the installed CLI version or falls back cleanly", () => {
  expect(cliVersion()).toMatch(/^\d+\.\d+\.\d+$/);
});

it("matches the CLI User-Agent template and honors overrides", () => {
  delete process.env["MY_ANTIGRAVITY_USER_AGENT"];
  expect(userAgent()).toMatch(
    /^antigravity\/cli\/\d+\.\d+\.\d+ \(aidev_client; os_type=(darwin|linux|windows); arch=(arm64|amd64|386|arm); purpose=agent; auth_method=consumer\)$/,
  );

  process.env["MY_ANTIGRAVITY_USER_AGENT"] = "antigravity/cli/9.9.9 (test)";
  expect(userAgent()).toBe("antigravity/cli/9.9.9 (test)");
});

it("reads OAuth client credentials from environment and throws when missing", () => {
  delete process.env["MY_ANTIGRAVITY_CLIENT_ID"];
  delete process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];
  delete process.env["ANTIGRAVITY_CLIENT_ID"];
  delete process.env["ANTIGRAVITY_CLIENT_SECRET"];
  expect(() => oauthClientConfig()).toThrow(/Missing OAuth client config/);

  process.env["MY_ANTIGRAVITY_CLIENT_ID"] = "cid";
  process.env["MY_ANTIGRAVITY_CLIENT_SECRET"] = "csec";
  expect(oauthClientConfig()).toStrictEqual({ clientId: "cid", clientSecret: "csec" });

  delete process.env["MY_ANTIGRAVITY_CLIENT_ID"];
  delete process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];
  process.env["ANTIGRAVITY_CLIENT_ID"] = "alt-id";
  process.env["ANTIGRAVITY_CLIENT_SECRET"] = "alt-sec";
  expect(oauthClientConfig()).toStrictEqual({ clientId: "alt-id", clientSecret: "alt-sec" });
});

it("sends the CLI header set in a stable order", () => {
  const headers = wireHeaders({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse",
    token: "t",
    body: '{"a":1}',
    accept: "text/event-stream",
  });
  expect(Object.keys(headers)).toStrictEqual([
    "authorization",
    "content-type",
    "accept",
    "accept-encoding",
    "user-agent",
    "content-length",
  ]);
  expect(headers["authorization"]).toBe("Bearer t");
  expect(headers["content-type"]).toBe("application/json");
  expect(headers["accept"]).toBe("text/event-stream");
  expect(headers["accept-encoding"]).toBe("gzip");
  expect(headers["content-length"]).toBe("7");

  const jsonHeaders = wireHeaders({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
    token: "t",
    body: "{}",
    accept: "application/json",
  });
  expect(jsonHeaders["accept"]).toBe("application/json");
});

it("offers HTTP/2 only, TLS 1.2+ and Go cipher preference order", () => {
  expect(TRANSPORT_TLS_OPTIONS.ALPNProtocols).toStrictEqual(["h2"]);
  expect(TRANSPORT_TLS_OPTIONS.minVersion).toBe("TLSv1.2");
  expect(TRANSPORT_TLS_OPTIONS.ciphers.split(":").slice(0, 2)).toStrictEqual([
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
  ]);
  expect(TRANSPORT_TLS_OPTIONS.ecdhCurve.startsWith("X25519")).toBe(true);
  expect(TRANSPORT_TLS_OPTIONS.sigalgs.startsWith("ecdsa_secp256r1_sha256")).toBe(true);
});
