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

  expect(await fetchUserInfo("t", { send, signal: new AbortController().signal })).toStrictEqual({
    email: "a@b.c",
    tier: "pro",
  });
  expect(attempts).toStrictEqual([
    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    "https://cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
  ]);
  expect(await loadCodeAssistProject("t", { send, signal: new AbortController().signal })).toBe(
    "aicode-consumers",
  );

  const directSend = async ({ url }: { url: string }): Promise<WireResponse> => {
    if (url.includes("fetchUserInfo")) {
      return jsonResponse(200, { email: "direct@b.c" });
    }
    return jsonResponse(200, { project: "fallback-proj" });
  };
  expect(await fetchUserInfo("t", { send: directSend })).toStrictEqual({ email: "direct@b.c" });
  expect(await loadCodeAssistProject("t", { send: directSend })).toBe("fallback-proj");

  const emptySend = async (): Promise<WireResponse> => jsonResponse(200, {});
  expect(await fetchUserInfo("t", { send: emptySend })).toStrictEqual({});
  expect(await loadCodeAssistProject("t", { send: emptySend })).toBeUndefined();

  const nonRecordSend = async (): Promise<WireResponse> => jsonResponse(200, "null");
  expect(await fetchUserInfo("t", { send: nonRecordSend })).toStrictEqual({});
  expect(await loadCodeAssistProject("t", { send: nonRecordSend })).toBeUndefined();
});

it("throws when all endpoints fail or return invalid payloads", async () => {
  const send = async (): Promise<WireResponse> => {
    throw new Error("network down");
  };
  await expect(fetchUserInfo("t", { send })).rejects.toThrow(/network down/);

  const nonErrorSend = async (): Promise<WireResponse> => {
    throw "raw-fail";
  };
  await expect(fetchUserInfo("t", { send: nonErrorSend })).rejects.toThrow(/raw-fail/);

  const emptyErrorSend = async (): Promise<WireResponse> => {
    throw new Error("");
  };
  await expect(fetchUserInfo("t", { send: emptyErrorSend })).rejects.toThrow(
    /no endpoint reachable/,
  );

  const arrayErrorSend = async (): Promise<WireResponse> => jsonResponse(403, "[]");
  await expect(loadCodeAssistProject("t", { send: arrayErrorSend })).rejects.toThrow(/403: \[\]/);

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
          quotaInfo: {
            remainingFraction: 0.25,
            resetTime: "2026-11-16T06:51:04Z",
          },
        },
        "claude-sonnet-5-5-high": {
          label: "Claude Sonnet 5.5",
          supportsThinking: true,
          quotaInfo: {
            resetTime: "2026-11-16T06:53:19Z",
          },
        },
        "gemini-3.8-flash-low": {
          modelName: "Gemini 3.8 Flash",
          quotaInfo: {},
        },
        "gpt-oss-120b-medium": {
          quotaInfo: "invalid",
        },
      },
    });

  const models = await fetchAvailableModels("t", "aicode-consumers", {
    send,
    signal: new AbortController().signal,
  });
  expect(models).toStrictEqual([
    {
      id: "claude-sonnet-5-5-high",
      displayName: "Claude Sonnet 5.5",
      supportsThinking: true,
      quota: {
        resetTime: "2026-11-16T06:53:19Z",
      },
    },
    {
      id: "gemini-3.8-flash-low",
      displayName: "Gemini 3.8 Flash",
    },
    {
      id: "gemini-4-hydrogen-medium",
      modelEnum: "MODEL_PLACEHOLDER_M526",
      displayName: "Gemini 4 Hydrogen (Medium)",
      supportsThinking: true,
      supportsImages: true,
      quota: {
        remainingFraction: 0.25,
        resetTime: "2026-11-16T06:51:04Z",
      },
    },
    {
      id: "gpt-oss-120b-medium",
    },
  ]);

  rememberModelEnums(models);
  expect(modelEnumFor("gemini-4-hydrogen-medium")).toBe("MODEL_PLACEHOLDER_M526");
  expect(modelEnumFor("claude-sonnet-5-5-high")).toBeUndefined();

  const emptyModelsSend = async (): Promise<WireResponse> => jsonResponse(200, {});
  expect(await fetchAvailableModels("t", "p", { send: emptyModelsSend })).toStrictEqual([]);
});
