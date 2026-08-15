import type { ExtensionAPI, ProviderConfig } from "@earendil-works/pi-coding-agent";
import { FALLBACK_CURSOR_MODELS, refreshCursorModels } from "./src/models.ts";
import { streamCursor } from "./src/provider.ts";

const provider: ProviderConfig = {
  name: "Cursor",
  baseUrl: "https://cursor.com",
  apiKey: "$CURSOR_API_KEY",
  api: "cursor-agent",
  models: FALLBACK_CURSOR_MODELS,
  refreshModels: refreshCursorModels,
  streamSimple: streamCursor,
};

export default function registerCursorProvider(pi: Pick<ExtensionAPI, "registerProvider">): void {
  pi.registerProvider("cursor", provider);
}
