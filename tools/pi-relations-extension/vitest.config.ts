import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    coverage: {
      // This is the entry wiring: it starts the PLaMo server through plamo-translate's own module,
      // Whose tests cover that code. Everything else here is measured.
      exclude: ["**/*.test.ts", "vitest.config.ts", "src/harness.ts", "src/live.ts"],
      include: ["index.ts", "src/**/*.ts"],
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
