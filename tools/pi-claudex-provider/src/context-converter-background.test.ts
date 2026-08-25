// This TypeScript file is executed with Bun.
import type { Api, Model } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { toPiContext } from "./context-converter.ts";
import type { StreamRequestMessage } from "./protocol.ts";

const MODEL: Model<Api> = {
  provider: "openai-codex",
  id: "gpt-5.6-luna",
  name: "GPT",
  api: "openai-codex-responses",
  baseUrl: "https://example.test",
  reasoning: true,
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1000,
  maxTokens: 100,
};
const REQUEST: StreamRequestMessage = {
  version: 1,
  type: "request",
  id: "request-1",
  token: "token",
  origin: "claudex",
  provider: "openai-codex",
  modelId: "gpt-5.6-luna",
  system: null,
  messages: [],
  tools: [{ name: "Bash", description: "Run shell commands", input_schema: null }],
  options: {},
};

it("teaches Pi-routed models the Claude Code background Bash lifecycle", () => {
  const context = toPiContext(REQUEST, MODEL);

  expect(context.tools?.[0]?.description).toMatch(
    /^Run shell commands[\s\S]*run_in_background=true[\s\S]*completion notification/u,
  );
});
