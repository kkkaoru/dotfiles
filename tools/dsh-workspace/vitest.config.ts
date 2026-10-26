import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    coverage: {
      // Cli.ts is the process boundary (real fs, spawn, fetch, timers).
      // Decisions it delegates to live in tested modules.
      exclude: ["**/*.test.ts", "vitest.config.ts", "src/cli.ts"],
      include: ["src/**/*.ts"],
      provider: "v8",
      reporter: ["text", "json-summary"],
      thresholds: {
        perFile: true,
        branches: 95,
        functions: 95,
        lines: 95,
        statements: 95,
      },
    },
    mockReset: true,
    restoreMocks: true,
  },
});
