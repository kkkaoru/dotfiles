// Runs with Bun; Vitest tests isolate external effects.
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    coverage: {
      provider: "v8",
      include: [
        "src/model.ts",
        "src/engine.ts",
        "src/storage.ts",
        "src/mcp-storage.ts",
        "src/vault.ts",
        "src/adapter.ts",
        "src/service.ts",
        "src/io.ts",
        "src/cli.ts",
      ],
      thresholds: { lines: 90, statements: 90, functions: 90, branches: 90 },
    },
  },
});
