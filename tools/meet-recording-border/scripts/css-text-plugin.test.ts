// @vitest-environment node
// Runs with bun: `bun run test` (vitest).
import { build } from "esbuild";
import { expect, test, vi } from "vitest";
import { minifiedCssText, minifyCss } from "./css-text-plugin.ts";

vi.mock("node:fs/promises", () => ({
  readFile: vi.fn(async () => "/* comment */\n.frame {\n  z-index: 2147483647;\n}\n"),
}));

test("minifyCss strips comments and whitespace but keeps large numbers exact", async () => {
  expect(await minifyCss("/* note */\n:host {\n  --layer: 2147483647;\n}\n")).toBe(
    ":host{--layer: 2147483647}\n",
  );
});

test("minifiedCssText inlines an imported stylesheet as minified text", async () => {
  const result = await build({
    stdin: {
      contents: 'import css from "./overlay.css"; export default css;',
      resolveDir: "src/overlay",
    },
    bundle: true,
    format: "esm",
    write: false,
    plugins: [minifiedCssText],
  });
  expect(result.outputFiles[0]?.text).toBe(
    '// src/overlay/overlay.css\nvar overlay_default = ".frame{z-index:2147483647}\\n";\n\n// <stdin>\nvar stdin_default = overlay_default;\nexport {\n  stdin_default as default\n};\n',
  );
});
