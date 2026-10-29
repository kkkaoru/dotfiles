// This TypeScript file is executed with Bun.
import { defineConfig } from 'vitest/config'

// The tests drive the hooks module directly with a fake engine: `claude plugin test` reports
// no coverage, so it is not used.
export default defineConfig({
  test: {
    include: ['hooks/**/*.test.ts'],
    coverage: {
      include: ['hooks/register.ts'],
      provider: 'v8',
      reporter: ['text', 'json-summary'],
      thresholds: { perFile: true, branches: 90, functions: 90, lines: 90, statements: 90 },
    },
  },
})
