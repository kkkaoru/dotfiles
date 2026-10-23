// Runs with bun: used by scripts/build.ts in `bun run build`, tested by `bun run test`.
// The esbuild minifier keeps numbers exact, unlike Bun's (2147483647 -> 2147480000).
import { readFile } from "node:fs/promises";
import { type Plugin, transform } from "esbuild";

// The esbuild onLoad filter is a Go regular expression, which rejects JavaScript flags.
// oxlint-disable-next-line require-unicode-regexp -- Go regular expressions have no u flag.
const CSS_FILE = /\.css$/;
const UTF8 = "utf8";

export const minifyCss = async (source: string): Promise<string> =>
  transform(source, { loader: "css", minify: true }).then((result) => result.code);

// Inlines imported CSS as minified text, matching the `*.css` module in @types/css.d.ts.
export const minifiedCssText: Plugin = {
  name: "minified-css-text",
  setup: (build) => {
    build.onLoad({ filter: CSS_FILE }, async ({ path }) => ({
      contents: await minifyCss(await readFile(path, UTF8)),
      loader: "text",
    }));
  },
};
