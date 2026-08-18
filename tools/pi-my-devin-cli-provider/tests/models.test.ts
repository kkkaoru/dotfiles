// This file runs with Bun.
import type { ExecFileOptionsWithStringEncoding } from "node:child_process";
import type { RefreshModelsContext } from "@earendil-works/pi-ai";
import { expect, test, vi } from "vitest";

type ExecCallback = (...parameters: [error: Error | null, stdout: string, stderr: string]) => void;

const execFileMock = vi.hoisted(() =>
  vi.fn<
    (
      ...parameters: [
        command: string,
        args: readonly string[],
        options: ExecFileOptionsWithStringEncoding,
        callback: ExecCallback,
      ]
    ) => void
  >(),
);

vi.mock("node:child_process", async (importOriginal) => {
  const original = await importOriginal<typeof import("node:child_process")>();
  return { ...original, execFile: execFileMock };
});

function refreshContext(): RefreshModelsContext {
  return {
    allowNetwork: false,
    publish: async () => true,
    signal: new AbortController().signal,
  };
}

test("provides the required stable fallback catalog with safety margins", async () => {
  const { FALLBACK_DEVIN_MODELS } = await import("../src/models.ts");

  expect(FALLBACK_DEVIN_MODELS.map((model) => model.id)).toStrictEqual([
    "adaptive",
    "swe-1-7",
    "swe-1-7-medium",
    "claude-sonnet-5-medium",
    "claude-opus-5-medium",
    "gpt-5-6-luna-medium",
    "gemini-3-7-flash-medium",
    "glm-5-2",
    "kimi-k3-high",
  ]);
  expect(FALLBACK_DEVIN_MODELS[1]?.contextWindow).toBe(209_600);
});

test("parses family variants, prices, defaults, and invalid entries", async () => {
  const { parseDevinModelCatalog } = await import("../src/models.ts");

  expect(
    parseDevinModelCatalog({
      families: [
        {
          variants: [
            {
              model_uid: "model-a",
              label: "Model A",
              max_context_tokens: 1_000_000,
              max_output_tokens: 64_000,
              cost_summary: "$2 / MTok In · $10 / MTok Out",
            },
            { model_uid: "model-b", label: "Model B" },
            { model_uid: "missing-label" },
            null,
          ],
        },
        {},
        null,
      ],
    }),
  ).toStrictEqual([
    {
      id: "model-a",
      name: "Model A",
      reasoning: true,
      input: ["text", "image"],
      cost: { input: 2, output: 10, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 800_000,
      maxTokens: 64_000,
    },
    {
      id: "model-b",
      name: "Model B",
      reasoning: true,
      input: ["text", "image"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 209_600,
      maxTokens: 128_000,
    },
  ]);
  expect(parseDevinModelCatalog(null)).toStrictEqual([]);
  expect(parseDevinModelCatalog({ families: "invalid" })).toStrictEqual([]);
});

test("refreshes models through the mocked Devin subprocess", async () => {
  execFileMock.mockImplementation((...parameters) => {
    const callback = parameters[3];
    callback(
      null,
      JSON.stringify({
        families: [{ variants: [{ model_uid: "live", label: "Live" }] }],
      }),
      "",
    );
  });
  const { refreshDevinModels } = await import("../src/models.ts");

  const models = await refreshDevinModels(refreshContext());

  expect(execFileMock).toHaveBeenCalledWith(
    "devin",
    ["models", "list", "--format", "json"],
    expect.objectContaining({ encoding: "utf8" }),
    expect.any(Function),
  );
  expect(models.map((model) => model.id)).toStrictEqual(["live"]);
});

test("falls back for an empty live catalog and rejects command errors", async () => {
  execFileMock.mockImplementationOnce((...parameters) => {
    const callback = parameters[3];
    callback(null, '{"families":[]}', "");
  });
  execFileMock.mockImplementationOnce((...parameters) => {
    const callback = parameters[3];
    callback(new Error("not authenticated"), "", "");
  });
  const { FALLBACK_DEVIN_MODELS, refreshDevinModels } = await import("../src/models.ts");

  await expect(refreshDevinModels(refreshContext())).resolves.toStrictEqual(FALLBACK_DEVIN_MODELS);
  await expect(refreshDevinModels(refreshContext())).rejects.toThrow("not authenticated");
});
