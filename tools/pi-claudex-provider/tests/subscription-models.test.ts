import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import {
  cachedSubscriptionModels,
  refreshSubscriptionModels,
  sdkSupportedModels,
  subscriptionCandidates,
} from "../src/subscription-models.ts";

interface CapturedQuery {
  readonly options: {
    readonly env: Record<string, string | undefined>;
    readonly abortController: AbortController;
  };
}

const queryMock = vi.hoisted(() => vi.fn());
vi.mock("@anthropic-ai/claude-agent-sdk", () => ({ query: queryMock }));

const roots: string[] = [];

async function tempDir(): Promise<string> {
  const root = await mkdtemp("/tmp/pi-claudex-subscription-");
  roots.push(root);
  return root;
}

afterEach(async () => {
  vi.unstubAllEnvs();
  queryMock.mockReset();
  await Promise.all(
    roots.splice(0).map(async (root) => {
      await rm(root, { recursive: true, force: true });
    }),
  );
});

it("keeps canonical Claude ids once and reads context windows from pi's catalog", () => {
  expect(
    subscriptionCandidates([
      { value: "default", resolvedModel: "claude-opus-5-5" },
      { value: "opus", resolvedModel: "claude-opus-5-5" },
      { value: "claude-haiku-4-5-20251001", resolvedModel: "claude-haiku-4-5-20251001" },
      { value: "claude-unlisted-9" },
      { value: "sonnet[1m]" },
    ]),
  ).toStrictEqual([
    { id: "claude-opus-5-5", contextWindow: 1_000_000 },
    { id: "claude-haiku-4-5-20251001", contextWindow: 200_000 },
    { id: "claude-unlisted-9", contextWindow: 200_000 },
  ]);
});

it("falls back to the built-in list when the cache is missing, invalid, or empty", async () => {
  const root = await tempDir();
  const missing = await cachedSubscriptionModels(path.join(root, "missing.json"));
  expect(missing.length).toBe(12);
  expect(missing[0]).toStrictEqual({ id: "claude-opus-5-5", contextWindow: 1_000_000 });
  const invalid = path.join(root, "invalid.json");
  await writeFile(invalid, "{not json");
  expect((await cachedSubscriptionModels(invalid)).length).toBe(12);
  const empty = path.join(root, "empty.json");
  await writeFile(empty, "[]");
  expect((await cachedSubscriptionModels(empty)).length).toBe(12);
});

it("reads a cached id list", async () => {
  const file = path.join(await tempDir(), "cache.json");
  await writeFile(file, '["claude-sonnet-5-5"]');
  expect(await cachedSubscriptionModels(file)).toStrictEqual([
    { id: "claude-sonnet-5-5", contextWindow: 1_000_000 },
  ]);
});

it("writes a live list to a new cache directory", async () => {
  const file = path.join(await tempDir(), "nested", "cache.json");
  const models = await refreshSubscriptionModels({
    list: async () => [{ value: "opus", resolvedModel: "claude-opus-5-5" }],
    cachePath: file,
    signal: undefined,
  });
  expect(models).toStrictEqual([{ id: "claude-opus-5-5", contextWindow: 1_000_000 }]);
  expect(await readFile(file, "utf8")).toBe('["claude-opus-5-5"]\n');
});

it("keeps the cached list when the live lookup fails or finds nothing", async () => {
  const file = path.join(await tempDir(), "cache.json");
  await writeFile(file, '["claude-opus-5"]');
  const failed = await refreshSubscriptionModels({
    list: async () => {
      throw new Error("not logged in");
    },
    cachePath: file,
    signal: undefined,
  });
  expect(failed).toStrictEqual([{ id: "claude-opus-5", contextWindow: 1_000_000 }]);
  const empty = await refreshSubscriptionModels({
    list: async () => [{ value: "default" }],
    cachePath: file,
    signal: undefined,
  });
  expect(empty).toStrictEqual([{ id: "claude-opus-5", contextWindow: 1_000_000 }]);
  expect(await readFile(file, "utf8")).toBe('["claude-opus-5"]');
});

it("asks the SDK under the user's own login and closes the session", async () => {
  vi.stubEnv("ANTHROPIC_API_KEY", "key");
  vi.stubEnv("ANTHROPIC_BASE_URL", "http://127.0.0.1:8318");
  vi.stubEnv("CLAUDE_CONFIG_DIR", "/tmp/isolated");
  vi.stubEnv("CLAUDECODE", "1");
  vi.stubEnv("CLAUDE_CODE_ENTRYPOINT", "cli");
  vi.stubEnv("KEEP_ME", "yes");
  const close = vi.fn();
  const captured: CapturedQuery[] = [];
  queryMock.mockImplementation((input: CapturedQuery) => {
    captured.push(input);
    return { supportedModels: async () => [{ value: "claude-opus-5-5" }], close };
  });
  const controller = new AbortController();
  expect(await sdkSupportedModels(controller.signal)).toStrictEqual([{ value: "claude-opus-5-5" }]);
  expect(close).toHaveBeenCalledTimes(1);
  const env = captured[0]?.options.env ?? {};
  expect([
    env["ANTHROPIC_API_KEY"],
    env["ANTHROPIC_BASE_URL"],
    env["CLAUDE_CONFIG_DIR"],
    env["CLAUDECODE"],
    env["CLAUDE_CODE_ENTRYPOINT"],
  ]).toStrictEqual([undefined, undefined, undefined, undefined, undefined]);
  expect(env["KEEP_ME"]).toBe("yes");
  controller.abort();
  expect(captured[0]?.options.abortController.signal.aborted).toBe(true);
});

it("closes the SDK session when the model lookup fails", async () => {
  const close = vi.fn();
  queryMock.mockImplementation(() => ({
    supportedModels: async () => {
      throw new Error("initialize failed");
    },
    close,
  }));
  await expect(sdkSupportedModels(undefined)).rejects.toThrow("initialize failed");
  expect(close).toHaveBeenCalledTimes(1);
});
