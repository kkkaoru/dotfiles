// This file runs with Bun.
import { expect, test } from "vitest";
import type { Api, Model } from "@earendil-works/pi-ai";
import { toPiContext } from "./context-converter.ts";

const MODEL: Model<Api> = {
  id: "offline",
  name: "Offline",
  provider: "offline",
  api: "openai-completions",
  baseUrl: "https://example.test",
  input: ["text"],
  reasoning: false,
  contextWindow: 1000,
  maxTokens: 100,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

test("rejects non-JSON tool arguments without silently changing input", () => {
  expect(() =>
    toPiContext(
      {
        version: 1,
        type: "request",
        id: "r1",
        token: "offline",
        origin: "claudex",
        provider: "offline",
        modelId: "offline",
        system: null,
        tools: [],
        options: {},
        messages: [
          {
            role: "assistant",
            content: [{ type: "tool_use", id: "call-1", name: "read", input: { bad: undefined } }],
          },
        ],
      },
      MODEL,
    ),
  ).toThrow("must contain only JSON values");
});
