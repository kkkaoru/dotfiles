import type { SDKModel } from "@cursor/sdk";
import type { RefreshModelsContext } from "@earendil-works/pi-ai";
import { beforeEach, expect, test, vi } from "vitest";
import {
  cursorCatalogToModels,
  cursorModelSelection,
  cursorModelsTestApi,
  FALLBACK_CURSOR_MODELS,
  loadCursorCatalog,
  recordCursorCatalog,
  refreshCursorModels,
  saveCursorCatalog,
} from "../src/models.ts";

const CATALOG: SDKModel[] = [
  {
    id: "default",
    displayName: "Auto",
    variants: [{ params: [], displayName: "Auto", isDefault: true }],
  },
  {
    id: "gpt-5.6-sol",
    displayName: "GPT-5.6 Sol",
    parameters: [
      {
        id: "reasoning",
        displayName: "Reasoning",
        values: ["none", "low", "medium", "high", "xhigh", "max"].map((value) => ({
          value,
        })),
      },
    ],
    variants: [
      {
        params: [
          { id: "context", value: "272k" },
          { id: "reasoning", value: "medium" },
          { id: "fast", value: "false" },
        ],
        displayName: "GPT-5.6 Sol",
        isDefault: true,
      },
      {
        params: [{ id: "context", value: "1m" }],
        displayName: "GPT-5.6 Sol Max",
      },
    ],
  },
];

const LUNA_CATALOG_MODEL: SDKModel = {
  id: "gpt-5.6-luna",
  displayName: "GPT-5.6 Luna",
  parameters: [
    {
      id: "context",
      displayName: "Context",
      values: [{ value: "272k" }, { value: "1m" }],
    },
    {
      id: "reasoning",
      displayName: "Reasoning",
      values: ["none", "low", "medium", "high", "xhigh", "max"].map((value) => ({
        value,
      })),
    },
    {
      id: "fast",
      displayName: "Fast",
      values: [{ value: "false" }, { value: "true" }],
    },
  ],
  variants: [
    {
      params: [
        { id: "context", value: "272k" },
        { id: "reasoning", value: "medium" },
        { id: "fast", value: "false" },
      ],
      displayName: "GPT-5.6 Luna",
    },
    {
      params: [
        { id: "context", value: "1m" },
        { id: "reasoning", value: "medium" },
        { id: "fast", value: "false" },
      ],
      displayName: "GPT-5.6 Luna",
      isDefault: true,
    },
  ],
};

function refreshContext(overrides: Partial<RefreshModelsContext> = {}): RefreshModelsContext {
  return {
    allowNetwork: false,
    publish: async () => true,
    signal: new AbortController().signal,
    ...overrides,
  };
}

beforeEach(async () => {
  await cursorModelsTestApi.resetCache();
});

test("provides multiple useful fallback models", () => {
  expect(FALLBACK_CURSOR_MODELS.map((model) => model.id)).toStrictEqual([
    "auto",
    "composer-2.5",
    "claude-sonnet-4-6",
    "claude-opus-5",
    "gpt-5.6-sol",
    "gpt-5.6-luna",
    "gpt-5.6-terra",
    "gpt-5.4",
    "gemini-3.1-pro",
    "grok-4.6",
    "kimi-k3",
    "glm-5.2",
  ]);
  expect(FALLBACK_CURSOR_MODELS).toContainEqual(
    expect.objectContaining({
      id: "gpt-5.6-sol",
      name: "GPT-5.6 Sol",
      contextWindow: 800_000,
      reasoning: true,
    }),
  );
  expect(FALLBACK_CURSOR_MODELS).toContainEqual(
    expect.objectContaining({
      id: "gpt-5.6-luna",
      name: "GPT-5.6 Luna",
      contextWindow: 217_600,
      reasoning: true,
    }),
  );
  expect(FALLBACK_CURSOR_MODELS).toContainEqual(
    expect.objectContaining({
      id: "gpt-5.6-terra",
      name: "GPT-5.6 Terra",
      contextWindow: 800_000,
      reasoning: true,
    }),
  );
});

test("converts the live Cursor catalog and context variants with a safety margin", () => {
  expect(cursorCatalogToModels(CATALOG)).toMatchObject([
    { id: "auto", name: "Auto", contextWindow: 204_800 },
    {
      id: "gpt-5.6-sol",
      name: "GPT-5.6 Sol",
      contextWindow: 800_000,
      reasoning: true,
    },
  ]);
});

test("uses Luna's 272K variant and enables Cursor fast", async () => {
  const catalog = [...CATALOG, LUNA_CATALOG_MODEL];

  expect(cursorCatalogToModels(catalog)).toContainEqual(
    expect.objectContaining({
      id: "gpt-5.6-luna",
      name: "GPT-5.6 Luna",
      contextWindow: 217_600,
      reasoning: true,
    }),
  );

  recordCursorCatalog(catalog);
  await expect(cursorModelSelection("gpt-5.6-luna", "max", "key")).resolves.toStrictEqual({
    id: "gpt-5.6-luna",
    params: [
      { id: "context", value: "272k" },
      { id: "reasoning", value: "max" },
      { id: "fast", value: "true" },
    ],
  });
});

test("applies Luna's context and fast overrides without an explicit effort", async () => {
  recordCursorCatalog([...CATALOG, LUNA_CATALOG_MODEL]);

  await expect(cursorModelSelection("gpt-5.6-luna", undefined, "key")).resolves.toStrictEqual({
    id: "gpt-5.6-luna",
    params: [
      { id: "context", value: "272k" },
      { id: "reasoning", value: "medium" },
      { id: "fast", value: "true" },
    ],
  });
});

test("adds auto when a live catalog omits it", () => {
  expect(cursorCatalogToModels(CATALOG.slice(1)).map((model) => model.id)).toStrictEqual([
    "auto",
    "gpt-5.6-sol",
  ]);
});

test("loads live effort capability on the first explicit-model request", async () => {
  const cursor = await import("@cursor/sdk");
  const list = vi.spyOn(cursor.Cursor.models, "list").mockResolvedValue(CATALOG);

  await expect(cursorModelSelection("gpt-5.6-sol", "high", "key")).resolves.toMatchObject({
    id: "gpt-5.6-sol",
    params: expect.arrayContaining([{ id: "reasoning", value: "high" }]),
  });
  expect(list).toHaveBeenCalledWith({ apiKey: "key" });
});

test("maps requested effort and enables Cursor fast in live default parameters", async () => {
  recordCursorCatalog(CATALOG);

  await expect(cursorModelSelection("gpt-5.6-sol", "max", "key")).resolves.toStrictEqual({
    id: "gpt-5.6-sol",
    params: [
      { id: "context", value: "272k" },
      { id: "reasoning", value: "max" },
      { id: "fast", value: "true" },
    ],
  });
});

test("warns instead of silently dropping unsupported auto effort", async () => {
  recordCursorCatalog(CATALOG);
  const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);

  await expect(cursorModelSelection("auto", "xhigh", "key")).resolves.toStrictEqual({
    id: "auto",
  });
  await cursorModelSelection("auto", "xhigh", "key");
  expect(warn).toHaveBeenCalledTimes(1);
  expect(warn).toHaveBeenCalledWith(
    "Cursor model auto does not expose a compatible effort parameter; requested xhigh was not forwarded.",
  );
});

test("uses fallbacks without network or credentials", async () => {
  expect(await refreshCursorModels(refreshContext())).toStrictEqual(FALLBACK_CURSOR_MODELS);
  expect(
    await refreshCursorModels(
      refreshContext({ allowNetwork: true, credential: { type: "api_key" } }),
    ),
  ).toStrictEqual(FALLBACK_CURSOR_MODELS);
});

test("fetches the authenticated live catalog", async () => {
  const cursor = await import("@cursor/sdk");
  const list = vi.spyOn(cursor.Cursor.models, "list").mockResolvedValue(CATALOG);

  const models = await refreshCursorModels(
    refreshContext({
      allowNetwork: true,
      credential: { type: "api_key", key: "key" },
    }),
  );

  expect(list).toHaveBeenCalledWith({ apiKey: "key" });
  expect(models.map((model) => model.id)).toStrictEqual(["auto", "gpt-5.6-sol"]);
});

test("saves and loads the catalog cache", async () => {
  await saveCursorCatalog("key", CATALOG);
  const cached = await loadCursorCatalog();
  expect(cached).toStrictEqual(CATALOG);
});

test("uses a fresh cached catalog when network is not allowed", async () => {
  await saveCursorCatalog("key", CATALOG);

  const models = await refreshCursorModels(refreshContext());

  expect(models.map((model) => model.id)).toStrictEqual(["auto", "gpt-5.6-sol"]);
});

test("uses a fresh cached catalog for model selection without a network call", async () => {
  const cursor = await import("@cursor/sdk");
  const list = vi.spyOn(cursor.Cursor.models, "list").mockResolvedValue(CATALOG);
  await saveCursorCatalog("key", CATALOG);

  const model = await cursorModelSelection("gpt-5.6-sol", "high", "key");

  expect(list).not.toHaveBeenCalled();
  expect(model).toMatchObject({
    id: "gpt-5.6-sol",
    params: expect.arrayContaining([{ id: "reasoning", value: "high" }]),
  });
});

test("falls back to hardcoded models when the cache is stale and network is not allowed", async () => {
  const now = Date.now();
  vi.spyOn(Date, "now").mockReturnValue(now - 2 * 60 * 60 * 1000);
  await saveCursorCatalog("key", CATALOG);
  vi.restoreAllMocks();

  const models = await refreshCursorModels(refreshContext());

  expect(models).toStrictEqual(FALLBACK_CURSOR_MODELS);
});
