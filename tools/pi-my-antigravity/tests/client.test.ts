// This file runs with bun.
import { expect, it } from "vitest";

import {
  fetchAvailableModels,
  fetchUserInfo,
  loadCodeAssistProject,
  modelEnumFor,
  rememberModelEnums,
} from "../src/client.ts";
import type { WireResponse } from "../src/transport.ts";

const jsonResponse = (status: number, body: unknown): WireResponse => ({
  status,
  headers: {},
  body: (async function* () {})(),
  text: async () => (typeof body === "string" ? body : JSON.stringify(body)),
});

it("fetches user info and loadCodeAssist project with endpoint fallback", async () => {
  const attempts: string[] = [];
  const send = async ({ url }: { url: string }): Promise<WireResponse> => {
    attempts.push(url);
    if (attempts.length === 1) {
      return jsonResponse(500, { error: { message: "boom" } });
    }
    if (url.includes("fetchUserInfo")) {
      return jsonResponse(200, { user: { email: "a@b.c" }, userTier: "pro" });
    }
    return jsonResponse(200, { cloudaicompanionProject: "aicode-consumers" });
  };

  expect(await fetchUserInfo("t", { send })).toStrictEqual({ email: "a@b.c", tier: "pro" });
  expect(attempts).toStrictEqual([
    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    "https://cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
  ]);
  expect(await loadCodeAssistProject("t", { send })).toBe("aicode-consumers");
});

it("throws when all endpoints fail or return invalid payloads", async () => {
  const send = async (): Promise<WireResponse> => {
    throw new Error("network down");
  };
  await expect(fetchUserInfo("t", { send })).rejects.toThrow(/network down/);

  const rawErrorSend = async (): Promise<WireResponse> => jsonResponse(403, "forbidden");
  await expect(loadCodeAssistProject("t", { send: rawErrorSend })).rejects.toThrow(
    /403: forbidden/,
  );
});

it("parses, filters, sorts, and remembers model enums from fetchAvailableModels", async () => {
  const send = async (): Promise<WireResponse> =>
    jsonResponse(200, {
      models: {
        chat_hidden: { displayName: "Hidden" },
        tab_hidden: { displayName: "Hidden" },
        "gemini-4-image": { displayName: "Image" },
        internal_only: { isInternal: true },
        "not-an-object": null,
        "gemini-4-hydrogen-medium": {
          model: "MODEL_PLACEHOLDER_M526",
          displayName: "Gemini 4 Hydrogen (Medium)",
          supportsThinking: true,
          supportsImages: true,
        },
        "claude-sonnet-5-5-high": {
          label: "Claude Sonnet 5.5",
          supportsThinking: true,
        },
      },
    });

  const models = await fetchAvailableModels("t", "aicode-consumers", { send });
  expect(models).toStrictEqual([
    {
      id: "claude-sonnet-5-5-high",
      displayName: "Claude Sonnet 5.5",
      supportsThinking: true,
    },
    {
      id: "gemini-4-hydrogen-medium",
      modelEnum: "MODEL_PLACEHOLDER_M526",
      displayName: "Gemini 4 Hydrogen (Medium)",
      supportsThinking: true,
      supportsImages: true,
    },
  ]);

  rememberModelEnums(models);
  expect(modelEnumFor("gemini-4-hydrogen-medium")).toBe("MODEL_PLACEHOLDER_M526");
  expect(modelEnumFor("claude-sonnet-5-5-high")).toBeUndefined();
});
