// This file runs with Bun.
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { invalidateDevinSessionsForPiSession } from "./runtime.ts";

const DEVIN_PROVIDER: string = "devin";

interface CompactionSessionContext {
  model: { provider?: string } | undefined;
  sessionManager: Pick<ExtensionContext["sessionManager"], "getSessionId">;
}

export function handleDevinCompactionEvent(ctx: CompactionSessionContext): void {
  if (ctx.model?.provider !== DEVIN_PROVIDER) return;
  invalidateDevinSessionsForPiSession(ctx.sessionManager.getSessionId());
}

export function registerDevinCompaction(pi: Pick<ExtensionAPI, "on">): void {
  pi.on("session_before_compact", (_event, ctx) => {
    handleDevinCompactionEvent(ctx);
  });
  pi.on("session_compact", (_event, ctx) => {
    handleDevinCompactionEvent(ctx);
  });
}
