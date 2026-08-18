// This file runs with Bun.
import type { ExtensionAPI, ProviderConfig } from "@earendil-works/pi-coding-agent";
import { registerDevinCompaction } from "./src/compaction.ts";
import { FALLBACK_DEVIN_MODELS, refreshDevinModels } from "./src/models.ts";
import { streamDevin } from "./src/provider.ts";

const provider: ProviderConfig = {
  name: "Devin CLI",
  baseUrl: "https://app.devin.ai",
  apiKey: "devin-cli-managed",
  api: "devin-cli-acp",
  models: FALLBACK_DEVIN_MODELS,
  refreshModels: refreshDevinModels,
  streamSimple: streamDevin,
};

export default function (pi: ExtensionAPI): void {
  registerDevinProvider(pi);
  registerDevinCompaction(pi);
}

export function registerDevinProvider(pi: Pick<ExtensionAPI, "registerProvider">): void {
  pi.registerProvider("devin", provider);
}
