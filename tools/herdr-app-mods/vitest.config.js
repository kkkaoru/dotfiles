// Tests the Herdr app mods under .config/herdr-app/mods with the host modules replaced by recorders.
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const mock = (name) => fileURLToPath(new URL(`./mocks/${name}.js`, import.meta.url));
const MODS = fileURLToPath(new URL("../../.config/herdr-app/mods/", import.meta.url));

export default defineConfig({
  resolve: {
    alias: { "gpui-kit": mock("gpui-kit"), "gpui-base": mock("gpui-base"), herdr: mock("herdr") },
  },
  test: {
    coverage: {
      provider: "v8",
      allowExternal: true,
      include: [`${MODS}*/*.js`],
      // Each mod file on its own: a well-tested file must not carry a weak one.
      thresholds: { perFile: true, statements: 95, branches: 95, functions: 95, lines: 95 },
    },
  },
});
