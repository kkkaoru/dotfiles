// This TypeScript file is executed with Bun.
import { createHash } from "node:crypto";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** The Cloudflare providers whose requests can land on any Workers AI instance. */
export const PROVIDERS = new Set(["cloudflare-workers-ai", "cloudflare-ai-gateway"]);

export const HEADER = "x-session-affinity";

/** What a headless `pi -p` run reports as its session id: the same value in every process. */
export const PLACEHOLDER_ID = "agent";

/**
 * The value Pi itself puts on every request, in every session. It pins all sessions to one
 * instance, which is why this extension has to replace it instead of deferring to it.
 */
export const PI_DEFAULT = "pi-agent";

/**
 * Workers AI reuses a prompt's prefill only on the instance that computed it, and this header is
 * what pins a session to one instance. Measured through the gateway: the same 3043-token prompt
 * reported 0 cached tokens alone and 2944 cached tokens with it (84 neurons against 5).
 */
export function affinityFor(provider: string | undefined, key: string): string | undefined {
  return provider !== undefined && PROVIDERS.has(provider) ? `pi-${key}` : undefined;
}

/**
 * Headless runs all answer `agent`, so taking that id at face value would pin every process to one
 * instance and make them evict each other. The session file stands in for it, and it survives a
 * resume of the same session.
 */
export function sessionKey(sessionId: string, file: string | undefined): string {
  if (sessionId !== "" && sessionId !== PLACEHOLDER_ID) {
    return sessionId;
  }
  if (file !== undefined) {
    return createHash("sha1").update(file).digest("hex").slice(0, 12);
  }
  return `pid-${String(process.pid)}`;
}

interface AffinityContext {
  readonly model?: { readonly provider: string };
  readonly sessionManager: {
    getSessionFile?: () => string | undefined;
    getSessionId: () => string;
  };
}

export default function cfAffinity(pi: ExtensionAPI): void {
  pi.on("before_provider_headers", (event, ctx) => {
    const context = ctx as AffinityContext;
    const key = sessionKey(
      context.sessionManager.getSessionId(),
      context.sessionManager.getSessionFile?.(),
    );
    const value = affinityFor(context.model?.provider, key);
    const existing = event.headers[HEADER];
    if (value === undefined || (existing !== undefined && existing !== PI_DEFAULT)) {
      return;
    }
    event.headers[HEADER] = value;
  });
}
