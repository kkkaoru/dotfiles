// Runs with Bun; Vitest executes tests in Node with mocked clocks and I/O.
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    coverage: {
      provider: "v8",
      include: ["index.ts", "src/**/*.ts"],
      exclude: ["src/**/*.test.ts"],
      thresholds: { lines: 90, functions: 90, statements: 90, branches: 90 },
    },
  },
});
