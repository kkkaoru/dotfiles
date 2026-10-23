// @vitest-environment node
// Runs with bun: `bun run test` (vitest).
import { build } from "esbuild";
import { expect, test, vi } from "vitest";

vi.mock("esbuild", () => ({ build: vi.fn(async () => ({ errors: [], warnings: [] })) }));

test("the build script bundles the content script into a minified dist/content.js", async () => {
  await import("./build.ts");
  const options = vi.mocked(build).mock.calls[0]?.[0];
  expect(options?.entryPoints).toStrictEqual(["src/content-script/content.ts"]);
  expect(options?.outfile).toBe("dist/content.js");
  expect(options?.bundle).toBe(true);
  expect(options?.minify).toBe(true);
  expect(options?.legalComments).toBe("none");
  expect(options?.plugins?.map((plugin) => plugin.name)).toStrictEqual(["minified-css-text"]);
});
