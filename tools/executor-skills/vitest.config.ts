// Runs with Bun; Vitest executes tests in Node.
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    coverage: {
      provider: "v8",
      include: ["catalog.ts", "server.ts"],
      thresholds: { lines: 90, functions: 90, statements: 90, branches: 90 },
    },
  },
});
