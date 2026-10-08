// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { providerSessionHeaders } from "./provider-session-headers.ts";

it("adds session headers only for OpenCode provider identities or hosts", () => {
  expect(providerSessionHeaders({ provider: "opencode", baseUrl: "invalid" }, "s1")).toStrictEqual({
    "x-opencode-session": "s1",
    "x-opencode-client": "pi",
    "User-Agent": "pi-coding-agent",
  });
  expect(
    providerSessionHeaders({ provider: "custom", baseUrl: "https://opencode.ai/zen" }, "s2"),
  ).toStrictEqual({
    "x-opencode-session": "s2",
    "x-opencode-client": "pi",
    "User-Agent": "pi-coding-agent",
  });
  expect(providerSessionHeaders({ provider: "other", baseUrl: "invalid" }, "s1")).toStrictEqual({});
  expect(
    providerSessionHeaders({ provider: "other", baseUrl: "https://example.test" }, "s1"),
  ).toStrictEqual({});
});
