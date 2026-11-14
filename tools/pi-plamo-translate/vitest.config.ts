import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    coverage: {
      exclude: ["**/*.test.ts", "index.ts", "vitest.config.ts"],
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
