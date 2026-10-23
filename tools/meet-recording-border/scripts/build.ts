// Runs with bun: `bun run build` bundles the content script into a minified dist/content.js.
import { type BuildOptions, build } from "esbuild";
import { minifiedCssText } from "./css-text-plugin.ts";

const BUILD_OPTIONS: BuildOptions = {
  entryPoints: ["src/content-script/content.ts"],
  outfile: "dist/content.js",
  bundle: true,
  minify: true,
  legalComments: "none",
  target: "chrome120",
  plugins: [minifiedCssText],
};

await build(BUILD_OPTIONS);
