// This file runs with bun.
import type { ExtensionAPI, ProviderModelConfig } from "@earendil-works/pi-coding-agent";

import { findCli, runCli } from "./src/cli.ts";
import { API_ID, PROVIDER_ID, PROVIDER_NAME } from "./src/constants.ts";
import { parseJson } from "./src/json.ts";
import { mergeDiscovered, parseModelList, staticModels } from "./src/models.ts";
import { streamAntigravity } from "./src/stream.ts";

/** The CLI's catalog, or the static one when it cannot be read. */
async function catalog(): Promise<ProviderModelConfig[]> {
  const binary = findCli();
  if (!binary) {
    return staticModels();
  }
  try {
    const { stdout } = await runCli(binary, ["models", "list", "--output-format", "json"]);
    const ids = parseModelList(parseJson(stdout));
    return ids.length > 0 ? mergeDiscovered(ids) : staticModels();
  } catch {
    return staticModels();
  }
}

export default function piMyAntigravity(pi: ExtensionAPI): void {
  const apiKey = process.env["GEMINI_API_KEY"];
  pi.registerProvider(PROVIDER_ID, {
    name: PROVIDER_NAME,
    api: API_ID,
    ...(apiKey ? { apiKey } : {}),
    streamSimple: streamAntigravity,
    models: staticModels(),
    refreshModels: catalog,
  });
}
