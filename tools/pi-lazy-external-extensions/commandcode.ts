// This file runs with Bun.

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { deferExternalExtension } from "./loader.ts";

export default function commandCodeExtension(pi: ExtensionAPI): void | Promise<void> {
  return deferExternalExtension(pi, "pi-commandcode-provider", "index.ts", "commandcode");
}
