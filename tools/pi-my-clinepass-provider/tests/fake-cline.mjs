#!/usr/bin/env node
import { createInterface } from "node:readline";

const mode = process.env.FAKE_CLINE_MODE;
const lines = createInterface({ input: process.stdin });
for await (const line of lines) {
  const request = JSON.parse(line);
  if (mode === "noise") {
    process.stdout.write("not-json\n");
    process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: 99, result: {} })}\n`);
  }
  if (request.id === 1) {
    const response =
      mode === "init-error"
        ? { jsonrpc: "2.0", id: 1, error: "invalid error" }
        : { jsonrpc: "2.0", id: 1, result: {} };
    process.stdout.write(`${JSON.stringify(response)}\n`);
    continue;
  }
  if (mode === "early-exit") {
    process.stderr.write("fixture stopped\n");
    process.exit(0);
  }
  if (mode === "hang") {
    continue;
  }
  if (mode === "rpc-error") {
    process.stdout.write(
      `${JSON.stringify({ jsonrpc: "2.0", id: 2, error: { message: "catalog failed" } })}\n`,
    );
    continue;
  }

  let result;
  if (mode === "invalid-result") {
    result = null;
  } else if (mode === "missing-models") {
    result = {};
  } else {
    let availableModels;
    if (mode === "malformed-catalog") {
      availableModels = [null, { modelId: "cline-pass/invalid" }];
    } else if (mode === "empty") {
      availableModels = [];
    } else {
      availableModels = [
        { modelId: "cline-pass/glm-5.3", name: "cline-pass/glm-5.3" },
        {
          modelId: "cline-pass/qwen3.8-max",
          name: "Qwen3.8 Max",
          description: "Latest",
        },
        { modelId: "other/model", name: "Other" },
        { modelId: 42, name: "Invalid" },
      ];
    }
    result = { models: { availableModels } };
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: 2, result })}\n`);
}
