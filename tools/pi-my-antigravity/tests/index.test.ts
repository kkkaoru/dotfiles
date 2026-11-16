// This file runs with bun.
import type { OAuthCredentials, RefreshModelsContext } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { expect, it, vi } from "vitest";

type RegisteredProviderConfig = Parameters<ExtensionAPI["registerProvider"]>[1];

vi.mock("../src/oauth.ts", () => ({
  login: vi.fn(async (onUrl: (url: string) => void) => {
    onUrl("https://accounts.google.com/o/oauth2/v2/auth?mock=1");
    return { type: "oauth", access: "acc-1", refresh: "ref-1", expires: Date.now() + 60_000 };
  }),
  refreshCredential: vi.fn(async (cred: { access: string }) => ({
    type: "oauth",
    access: `${cred.access}-refreshed`,
    refresh: "ref-1",
    expires: Date.now() + 60_000,
  })),
}));

vi.mock("../src/client.ts", async () => {
  const actual = await vi.importActual<typeof import("../src/client.ts")>("../src/client.ts");
  return {
    ...actual,
    loadCodeAssistProject: vi.fn(async () => "aicode-consumers"),
    fetchUserInfo: vi.fn(async () => ({ email: "user@example.com" })),
    fetchQuotaSummary: vi.fn(async () => ({ groups: [] })),
    fetchAvailableModels: vi.fn(async () => [
      { id: "gemini-4-hydrogen-medium", modelEnum: "MODEL_PLACEHOLDER_M526" },
      { id: "gemini-5-new-high", displayName: "Gemini 5 New" },
    ]),
  };
});

vi.mock("../src/transport.ts", async () => {
  const actual = await vi.importActual<typeof import("../src/transport.ts")>("../src/transport.ts");
  return {
    ...actual,
    closePool: vi.fn(),
  };
});

import piMyAntigravity from "../index.ts";
import * as clientModule from "../src/client.ts";
import { closePool } from "../src/transport.ts";

interface CapturedRegistry {
  providerId?: string;
  config?: RegisteredProviderConfig;
  events: Map<string, () => Promise<void> | void>;
  commands: Map<string, (args: string, ctx: unknown) => Promise<void>>;
  sentMessages: Array<{ message: { customType: string; content: string; display: boolean } }>;
  emittedEvents: Array<{ event: string; data: unknown }>;
}

function setupExtension(): CapturedRegistry {
  const captured: CapturedRegistry = {
    events: new Map(),
    commands: new Map(),
    sentMessages: [],
    emittedEvents: [],
  };
  const pi = {
    registerProvider(id: string, config: RegisteredProviderConfig) {
      captured.providerId = id;
      captured.config = config;
    },
    on(event: string, handler: () => Promise<void> | void) {
      captured.events.set(event, handler);
    },
    registerCommand(
      name: string,
      opts: { handler: (args: string, ctx: unknown) => Promise<void> },
    ) {
      captured.commands.set(name, opts.handler);
    },
    sendMessage(message: { customType: string; content: string; display: boolean }) {
      captured.sentMessages.push({ message });
    },
    events: {
      emit(event: string, data: unknown) {
        captured.emittedEvents.push({ event, data });
      },
    },
  } as unknown as ExtensionAPI;

  piMyAntigravity(pi);
  return captured;
}

it("registers provider, OAuth login/refresh/getApiKey, refreshModels, and shutdown hook", async () => {
  const captured = setupExtension();
  expect(captured.providerId).toBe("my-antigravity");
  expect(captured.config?.oauth?.isSubscription).toBe(true);

  // Before any login or refresh, getApiKey without projectId and doctor show unresolved state.
  expect(
    captured.config!.oauth!.getApiKey({ access: "acc-0" } as unknown as OAuthCredentials),
  ).toBe('{"token":"acc-0"}');
  await captured.commands.get("my-antigravity.doctor")!("", {
    hasUI: false,
    ui: { notify: () => {} },
    modelRegistry: { getApiKeyForProvider: async () => undefined },
  });

  // Login when loadCodeAssistProject returns undefined and fetchUserInfo returns no email.
  vi.mocked(clientModule.loadCodeAssistProject).mockResolvedValueOnce(undefined);
  vi.mocked(clientModule.fetchUserInfo).mockResolvedValueOnce({});
  const emptyMetaLogin = await captured.config!.oauth!.login({
    onAuth: () => {},
    onDeviceCode: () => {},
    onPrompt: async () => "",
    onSelect: async () => undefined,
  });
  expect(emptyMetaLogin["projectId"]).toBeUndefined();
  expect(emptyMetaLogin["email"]).toBeUndefined();

  // Refresh before project is cached exercises projectFor() when PROJECT_STATE.resolved is undefined.
  const preLoginRefresh = await captured.config!.refreshModels!({
    credential: { type: "oauth", access: "acc-pre", refresh: "ref-pre", expires: 0 },
    allowNetwork: true,
    signal: new AbortController().signal,
    publish: async () => true,
  });
  expect(preLoginRefresh.length).toBe(8);

  const authPrompts: string[] = [];
  const loggedIn = await captured.config!.oauth!.login({
    onAuth: ({ url }) => authPrompts.push(url),
    onDeviceCode: () => {},
    onPrompt: async () => "",
    onSelect: async () => undefined,
  });
  expect(authPrompts).toStrictEqual(["https://accounts.google.com/o/oauth2/v2/auth?mock=1"]);
  expect(loggedIn["projectId"]).toBe("aicode-consumers");
  expect(loggedIn["email"]).toBe("user@example.com");

  expect(captured.config!.oauth!.getApiKey(loggedIn)).toBe(
    '{"token":"acc-1","projectId":"aicode-consumers"}',
  );
  expect(captured.config!.oauth!.getApiKey({} as OAuthCredentials)).toBe("");

  const refreshed = await captured.config!.oauth!.refreshToken(
    loggedIn,
    new AbortController().signal,
  );
  expect(refreshed.access).toBe("acc-1-refreshed");

  const offlineModels = await captured.config!.refreshModels!({
    credential: loggedIn as unknown as NonNullable<RefreshModelsContext["credential"]>,
    allowNetwork: false,
    signal: new AbortController().signal,
    publish: async () => true,
  });
  expect(offlineModels.length).toBe(7);

  const liveModels = await captured.config!.refreshModels!({
    credential: loggedIn as unknown as NonNullable<RefreshModelsContext["credential"]>,
    allowNetwork: true,
    signal: new AbortController().signal,
    publish: async () => true,
  });
  expect(liveModels.map((model) => model.id).at(-1)).toBe("gemini-5-new");

  await captured.events.get("session_shutdown")?.();
  expect(closePool).toHaveBeenCalledTimes(1);
});

it("handles my-antigravity.models and my-antigravity.doctor commands", async () => {
  const captured = setupExtension();
  const notices: Array<{ text: string; level: string }> = [];
  const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});

  const noCredCtx = {
    hasUI: true,
    ui: { notify: (text: string, level: string) => notices.push({ text, level }) },
    modelRegistry: { getApiKeyForProvider: async () => undefined },
  };
  await captured.commands.get("my-antigravity.models")!("", noCredCtx);
  expect(notices[0]?.level).toBe("warning");

  const credCtx = {
    hasUI: true,
    ui: { notify: (text: string, level: string) => notices.push({ text, level }) },
    modelRegistry: {
      getApiKeyForProvider: async () => '{"token":"acc-1"}',
    },
  };
  await captured.commands.get("my-antigravity.models")!("", credCtx);
  await captured.commands.get("my-antigravity.quota")!("", noCredCtx);
  await captured.commands.get("my-antigravity.quota")!("", credCtx);
  expect(captured.sentMessages.at(-1)?.message.customType).toBe("my-antigravity-quota");
  expect(captured.sentMessages.at(-1)?.message.content).toContain("No quota info returned");

  vi.mocked(clientModule.fetchAvailableModels).mockResolvedValueOnce([
    {
      id: "gemini-4-hydrogen-medium",
      quota: { remainingFraction: 0.0363, resetTime: "2026-11-16T06:51:04Z" },
    },
    {
      id: "claude-sonnet-5-5-high",
      quota: { resetTime: "invalid-date" },
    },
    {
      id: "gemini-4-argon-low",
      quota: { remainingFraction: 0.99 },
    },
  ]);
  await captured.commands.get("my-antigravity.quota")!("", credCtx);
  const quotaMessage = captured.sentMessages.at(-1)?.message.content ?? "";
  expect(captured.emittedEvents.at(-1)).toStrictEqual({
    event: "plamo-translate:exclude",
    data: quotaMessage,
  });
  expect(quotaMessage).toContain(
    "gemini-4-hydrogen-medium: 3.6% remaining, resets 2026-11-16 15:51:04 JST",
  );
  expect(quotaMessage).toContain("claude-sonnet-5-5-high: remaining unknown, resets invalid-date");
  expect(quotaMessage).toContain("gemini-4-argon-low: 99.0% remaining");

  vi.mocked(clientModule.fetchQuotaSummary).mockResolvedValueOnce({
    groups: [
      {
        displayName: "Gemini Models",
        buckets: [
          {
            bucketId: "gemini-weekly",
            window: "weekly",
            remainingFraction: 0.58,
            resetTime: "2026-11-21T14:59:37Z",
          },
          {
            bucketId: "gemini-5h",
            window: "5h",
            remainingFraction: 0.58,
            resetTime: "2026-11-16T11:51:04Z",
          },
        ],
      },
      {
        displayName: "Claude and GPT models",
        buckets: [
          {
            bucketId: "3p-weekly",
            window: "weekly",
            remainingFraction: 0.886,
            resetTime: "2026-11-23T01:53:19Z",
          },
          {
            bucketId: "3p-5h",
            window: "5h",
            remainingFraction: 0.999,
            resetTime: "2026-11-16T12:43:31Z",
          },
        ],
      },
      {
        displayName: "Gemini 4 Argon",
        buckets: [
          {
            bucketId: "gemini-argon-wtu-weekly",
            window: "weekly",
            remainingFraction: 0.301,
            resetTime: "2026-11-21T14:58:47Z",
          },
        ],
      },
    ],
  });
  await captured.commands.get("my-antigravity.quota")!("", credCtx);
  const summaryMessage = captured.sentMessages.at(-1)?.message.content ?? "";
  expect(summaryMessage).toContain("Gemini Models\n  weekly: 58.0% remaining");
  expect(summaryMessage).toContain("Claude and GPT models\n  weekly: 88.6% remaining");
  expect(summaryMessage).toContain("Gemini 4 Argon\n  weekly: 30.1% remaining");

  vi.mocked(clientModule.fetchQuotaSummary).mockResolvedValueOnce({
    groups: [
      {
        displayName: "Empty Group",
        buckets: [{ bucketId: "fallback-bucket" }],
      },
    ],
  });
  await captured.commands.get("my-antigravity.quota")!("", credCtx);
  expect(captured.sentMessages.at(-1)?.message.content).toContain(
    "Empty Group\n  fallback-bucket: remaining unknown",
  );

  vi.mocked(clientModule.fetchQuotaSummary).mockRejectedValueOnce(new Error("summary boom"));
  vi.mocked(clientModule.fetchAvailableModels).mockRejectedValueOnce(new Error("models boom"));
  await expect(captured.commands.get("my-antigravity.quota")!("", credCtx)).rejects.toThrow(
    /summary boom/,
  );

  await captured.commands.get("my-antigravity.doctor")!("", credCtx);
  expect(logSpy).toHaveBeenCalled();

  const noProjRefresh = await captured.config!.refreshModels!({
    credential: {
      type: "oauth",
      access: "acc-1",
      refresh: "ref-1",
      expires: 0,
    },
    allowNetwork: true,
    signal: new AbortController().signal,
    publish: async () => true,
  });
  expect(noProjRefresh.length).toBe(8);
  expect(
    captured.config!.oauth!.getApiKey({ access: "acc-1" } as unknown as OAuthCredentials),
  ).toBe('{"token":"acc-1","projectId":"aicode-consumers"}');

  vi.mocked(clientModule.fetchAvailableModels).mockRejectedValueOnce(new Error("catalog boom"));
  await expect(
    captured.config!.refreshModels!({
      credential: {
        type: "oauth",
        access: "acc-1",
        refresh: "ref-1",
        expires: 0,
      },
      allowNetwork: true,
      signal: new AbortController().signal,
      publish: async () => true,
    }),
  ).rejects.toThrow(/catalog boom/);

  const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.mocked(clientModule.loadCodeAssistProject).mockRejectedValueOnce(new Error("proj fail"));
  vi.mocked(clientModule.fetchUserInfo).mockRejectedValueOnce(new Error("user fail"));
  const partialLogin = await captured.config!.oauth!.login({
    onAuth: () => {},
    onDeviceCode: () => {},
    onPrompt: async () => "",
    onSelect: async () => undefined,
  });
  expect(partialLogin.access).toBe("acc-1");
  expect(warnSpy).toHaveBeenCalledTimes(2);

  vi.mocked(clientModule.loadCodeAssistProject).mockRejectedValueOnce("raw-proj-fail");
  vi.mocked(clientModule.fetchUserInfo).mockRejectedValueOnce("raw-user-fail");
  const stringErrLogin = await captured.config!.oauth!.login({
    onAuth: () => {},
    onDeviceCode: () => {},
    onPrompt: async () => "",
    onSelect: async () => undefined,
  });
  expect(stringErrLogin.access).toBe("acc-1");
  expect(warnSpy).toHaveBeenCalledTimes(4);

  vi.mocked(clientModule.loadCodeAssistProject).mockResolvedValueOnce(undefined);
  const cachedRefresh = await captured.config!.refreshModels!({
    credential: { type: "oauth", access: "acc-2", refresh: "ref-2", expires: 0 },
    allowNetwork: true,
    signal: new AbortController().signal,
    publish: async () => true,
  });
  expect(cachedRefresh.length).toBe(8);

  const noCredRefresh = await captured.config!.refreshModels!({
    allowNetwork: true,
    signal: new AbortController().signal,
    publish: async () => true,
  });
  expect(noCredRefresh.length).toBe(7);

  const silentCtx = {
    hasUI: false,
    ui: { notify: () => {} },
    modelRegistry: { getApiKeyForProvider: async () => '{"token":"acc-1"}' },
  };
  await captured.commands.get("my-antigravity.models")!("", silentCtx);
  await captured.commands.get("my-antigravity.quota")!("", silentCtx);
  await captured.commands.get("my-antigravity.doctor")!("", silentCtx);
});
