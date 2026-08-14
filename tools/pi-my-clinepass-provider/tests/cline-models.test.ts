import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { discoverClinePassModels } from "../src/cline-models.ts";

const FAKE_CLINE = fileURLToPath(new URL("fake-cline.mjs", import.meta.url));

describe("Cline ACP model discovery", () => {
  it("returns only valid ClinePass models", async () => {
    const models = await discoverClinePassModels({ command: FAKE_CLINE });
    expect(models).toStrictEqual([
      { modelId: "cline-pass/glm-5.3", name: "cline-pass/glm-5.3" },
      {
        description: "Latest",
        modelId: "cline-pass/qwen3.8-max",
        name: "Qwen3.8 Max",
      },
    ]);
  });

  it("ignores unrelated and malformed ACP output", async () => {
    const models = await discoverClinePassModels({
      command: FAKE_CLINE,
      env: { FAKE_CLINE_MODE: "noise" },
    });
    expect(models.map((model) => model.modelId)).toStrictEqual([
      "cline-pass/glm-5.3",
      "cline-pass/qwen3.8-max",
    ]);
  });

  it("surfaces initialization errors", async () => {
    await expect(
      discoverClinePassModels({
        command: FAKE_CLINE,
        env: { FAKE_CLINE_MODE: "init-error" },
      }),
    ).rejects.toThrow("cline ACP initialize failed: unknown ACP error");
  });

  it("surfaces ACP errors", async () => {
    await expect(
      discoverClinePassModels({
        command: FAKE_CLINE,
        env: { FAKE_CLINE_MODE: "rpc-error" },
      }),
    ).rejects.toThrow("catalog failed");
  });

  it("rejects an empty ClinePass catalog", async () => {
    await expect(
      discoverClinePassModels({ command: FAKE_CLINE, env: { FAKE_CLINE_MODE: "empty" } }),
    ).rejects.toThrow("returned no ClinePass models");
  });

  it("rejects invalid session results", async () => {
    await expect(
      discoverClinePassModels({
        command: FAKE_CLINE,
        env: { FAKE_CLINE_MODE: "invalid-result" },
      }),
    ).rejects.toThrow("returned no ClinePass models");
    await expect(
      discoverClinePassModels({
        command: FAKE_CLINE,
        env: { FAKE_CLINE_MODE: "missing-models" },
      }),
    ).rejects.toThrow("returned no ClinePass models");
    await expect(
      discoverClinePassModels({
        command: FAKE_CLINE,
        env: { FAKE_CLINE_MODE: "malformed-catalog" },
      }),
    ).rejects.toThrow("returned no ClinePass models");
  });

  it("reports early process exit diagnostics", async () => {
    await expect(
      discoverClinePassModels({
        command: FAKE_CLINE,
        env: { FAKE_CLINE_MODE: "early-exit" },
      }),
    ).rejects.toThrow("fixture stopped");
  });

  it("terminates an unresponsive Cline ACP process", async () => {
    await expect(
      discoverClinePassModels({
        command: FAKE_CLINE,
        env: { FAKE_CLINE_MODE: "hang" },
        timeoutMs: 20,
      }),
    ).rejects.toThrow("exited before response 2");
  });

  it("rejects a missing cline executable", async () => {
    await expect(
      discoverClinePassModels({ command: "/missing/cline-command", timeoutMs: 100 }),
    ).rejects.toThrow();
  });
});
