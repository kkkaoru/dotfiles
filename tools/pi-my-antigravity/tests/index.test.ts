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
}

function setupExtension(): CapturedRegistry {
  const captured: CapturedRegistry = {
    events: new Map(),
    commands: new Map(),
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
  await captured.commands.get("my-antigravity.doctor")!("", silentCtx);
});
