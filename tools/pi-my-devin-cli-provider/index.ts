// This file runs with Bun.
import {
  lazyStream,
  type Api,
  type AssistantMessageEventStream,
  type Context,
  type Model,
  type SimpleStreamOptions,
} from "@earendil-works/pi-ai";
import type { ExtensionAPI, ProviderConfig } from "@earendil-works/pi-coding-agent";
import { registerDevinCompaction } from "./src/compaction.ts";
import { FALLBACK_DEVIN_MODELS, refreshDevinModels } from "./src/models.ts";

function lazyStreamDevin(
  model: Model<Api>,
  context: Context,
  options?: SimpleStreamOptions,
): AssistantMessageEventStream {
  return lazyStream(model, async () => {
    const module = await import("./src/provider.ts");
    return module.streamDevin(model, context, options);
  });
}

const provider: ProviderConfig = {
  name: "Devin CLI",
  baseUrl: "https://app.devin.ai",
  apiKey: "devin-cli-managed",
  api: "devin-cli-acp",
  models: FALLBACK_DEVIN_MODELS,
  refreshModels: refreshDevinModels,
  streamSimple: lazyStreamDevin,
};

export default function (pi: ExtensionAPI): void {
  registerDevinProvider(pi);
  registerDevinCompaction(pi);
}

export function registerDevinProvider(pi: Pick<ExtensionAPI, "registerProvider">): void {
  pi.registerProvider("devin", provider);
}
