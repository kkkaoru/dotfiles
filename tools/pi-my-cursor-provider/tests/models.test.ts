import type { SDKModel } from "@cursor/sdk";
import type { RefreshModelsContext } from "@earendil-works/pi-ai";
import { expect, test, vi } from "vitest";
import {
  cursorCatalogToModels,
  FALLBACK_CURSOR_MODELS,
  refreshCursorModels,
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
    variants: [
      {
        params: [{ id: "context", value: "272k" }],
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

function refreshContext(overrides: Partial<RefreshModelsContext> = {}): RefreshModelsContext {
  return {
    allowNetwork: false,
    publish: async () => true,
    signal: new AbortController().signal,
    ...overrides,
  };
}

test("provides multiple useful fallback models", () => {
  expect(FALLBACK_CURSOR_MODELS.map((model) => model.id)).toStrictEqual([
    "auto",
    "composer-2.5",
    "claude-sonnet-4-6",
    "claude-opus-5",
    "gpt-5.6-sol",
    "gpt-5.4",
    "gemini-3.1-pro",
    "grok-4.6",
    "kimi-k3",
    "glm-5.2",
  ]);
});

test("converts the live Cursor catalog and context variants", () => {
  expect(cursorCatalogToModels(CATALOG)).toMatchObject([
    { id: "auto", name: "Auto", contextWindow: 256_000 },
    { id: "gpt-5.6-sol", name: "GPT-5.6 Sol", contextWindow: 1_000_000 },
  ]);
});

test("adds auto when a live catalog omits it", () => {
  expect(cursorCatalogToModels(CATALOG.slice(1)).map((model) => model.id)).toStrictEqual([
    "auto",
    "gpt-5.6-sol",
  ]);
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
