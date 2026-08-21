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
import { registerCursorCompaction } from "./src/compaction.ts";
import { FALLBACK_CURSOR_MODELS, refreshCursorModels } from "./src/models.ts";

function lazyStreamCursor(
  model: Model<Api>,
  context: Context,
  options?: SimpleStreamOptions,
): AssistantMessageEventStream {
  return lazyStream(model, async () => {
    const module = await import("./src/provider.ts");
    return module.streamCursor(model, context, options);
  });
}

const provider: ProviderConfig = {
  name: "Cursor",
  baseUrl: "https://cursor.com",
  apiKey: "$CURSOR_API_KEY",
  api: "cursor-agent",
  models: FALLBACK_CURSOR_MODELS,
  refreshModels: refreshCursorModels,
  streamSimple: lazyStreamCursor,
};

export default function (pi: ExtensionAPI): void {
  registerCursorProvider(pi);
  registerCursorCompaction(pi);
}

export function registerCursorProvider(pi: Pick<ExtensionAPI, "registerProvider">): void {
  pi.registerProvider("cursor", provider);
}
