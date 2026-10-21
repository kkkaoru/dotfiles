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
import { CLAUDE_SDK_MODELS } from "./src/models.ts";

export const CLAUDE_SDK_PROVIDER_ID = "claude-sdk";

function lazyStreamClaudeSdk(
  model: Model<Api>,
  context: Context,
  options?: SimpleStreamOptions,
): AssistantMessageEventStream {
  // The SDK and its native Claude Code binary load only when this provider is used.
  return lazyStream(model, async () => {
    const module = await import("./src/provider.ts");
    return module.streamClaudeSdk(model, context, options);
  });
}

const provider: ProviderConfig = {
  name: "Claude Agent SDK",
  baseUrl: "https://api.anthropic.com",
  // The SDK uses the local Claude Code login; this marker only satisfies registration.
  apiKey: "claude-agent-sdk-managed",
  api: "claude-agent-sdk",
  models: CLAUDE_SDK_MODELS,
  streamSimple: lazyStreamClaudeSdk,
};

export function registerClaudeSdkProvider(pi: Pick<ExtensionAPI, "registerProvider">): void {
  pi.registerProvider(CLAUDE_SDK_PROVIDER_ID, provider);
}

export default function claudeSdkExtension(pi: Pick<ExtensionAPI, "registerProvider">): void {
  registerClaudeSdkProvider(pi);
}
