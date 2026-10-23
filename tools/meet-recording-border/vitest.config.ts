// Runs with bun: `bun run test`.
import { readFile } from "node:fs/promises";
import { defineConfig } from "vitest/config";

// Mirrors scripts/css-text-plugin.ts: CSS imported from TypeScript becomes its source text.
// The suffix has no "?" so Vite's own CSS pipeline, which matches `.css` and `.css?…`, skips it.
const TEXT_SUFFIX = ".text";

export default defineConfig({
  plugins: [
    {
      name: "css-text",
      enforce: "pre",
      resolveId: async function resolveId(source, importer): Promise<string | null> {
        const resolved =
          source.endsWith(".css") && importer !== undefined
            ? await this.resolve(source, importer, { skipSelf: true })
            : null;
        return resolved === null ? null : `\0${resolved.id}${TEXT_SUFFIX}`;
      },
      load: async (id): Promise<string | null> =>
        id.endsWith(TEXT_SUFFIX)
          ? `export default ${JSON.stringify(await readFile(id.slice(1, -TEXT_SUFFIX.length), "utf8"))};`
          : null,
    },
  ],
  test: {
    coverage: {
      exclude: ["**/*.test.ts", "**/*.d.ts", "vitest.config.ts"],
      include: ["scripts/**/*.ts", "src/**/*.ts", "stylelint-plugin/**/*.ts"],
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
    environment: "happy-dom",
    mockReset: true,
    restoreMocks: true,
  },
});
