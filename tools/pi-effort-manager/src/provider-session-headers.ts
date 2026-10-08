// This TypeScript file is executed with Bun.
import { URL } from "node:url";
import type { ProviderHeaders } from "@earendil-works/pi-ai";

export interface SessionHeaderTarget {
  readonly provider: string;
  readonly baseUrl: string;
}
const OPENCODE_HOST = "opencode.ai";
const OPENCODE_CLIENT = "pi-coding-agent";
const OPENCODE_PROVIDERS: ReadonlySet<string> = new Set(["opencode", "opencode-go"]);

function hostOf(baseUrl: string): string | undefined {
  try {
    return new URL(baseUrl).hostname;
  } catch {
    return undefined;
  }
}

/** Extension model calls bypass Pi's provider runner and need its OpenCode session headers. */
export function providerSessionHeaders(
  model: SessionHeaderTarget,
  sessionId: string,
): ProviderHeaders {
  if (!OPENCODE_PROVIDERS.has(model.provider) && hostOf(model.baseUrl) !== OPENCODE_HOST) {
    return {};
  }
  return {
    "x-opencode-session": sessionId,
    "x-opencode-client": "pi",
    "User-Agent": OPENCODE_CLIENT,
  };
}
