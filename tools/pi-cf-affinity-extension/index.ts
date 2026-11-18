// This TypeScript file is executed with Bun.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** The Cloudflare providers whose requests can land on any Workers AI instance. */
export const PROVIDERS = new Set(["cloudflare-workers-ai", "cloudflare-ai-gateway"]);

export const HEADER = "x-session-affinity";

/**
 * Workers AI reuses a prompt's prefill only on the instance that computed it, and this header is
 * what pins a session to one instance. Measured through the gateway: the same 3043-token prompt
 * reported 0 cached tokens alone and 2944 cached tokens with it (84 neurons against 5).
 */
export function affinityFor(provider: string | undefined, sessionId: string): string | undefined {
  return provider !== undefined && PROVIDERS.has(provider) ? `pi-${sessionId}` : undefined;
}

export default function cfAffinity(pi: ExtensionAPI): void {
  pi.on("before_provider_headers", (event, ctx) => {
    const value = affinityFor(ctx.model?.provider, ctx.sessionManager.getSessionId());
    if (value === undefined || event.headers[HEADER] !== undefined) {
      return;
    }
    event.headers[HEADER] = value;
  });
}
