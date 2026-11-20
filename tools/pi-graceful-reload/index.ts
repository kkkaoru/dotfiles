// This TypeScript file is executed with Bun.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** A tool that ignores the abort keeps the run alive, and reloading then throws; give up waiting. */
export const SETTLE_TIMEOUT_MS = 3000;

function settleTimeout(): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, SETTLE_TIMEOUT_MS).unref();
  });
}

export default function gracefulReload(pi: ExtensionAPI): void {
  pi.registerCommand("graceful-reload", {
    description:
      "Reload keybindings, extensions, skills, prompts, themes, and context files, aborting a running response instead of waiting for it",
    async handler(_args, ctx) {
      // The built-in /reload refuses while a response or compaction is live. Aborting keeps the
      // partial answer in the transcript and lets the reload happen now.
      if (!ctx.isIdle()) {
        ctx.abort();
        await Promise.race([ctx.waitForIdle(), settleTimeout()]);
      }
      try {
        await ctx.reload();
      } catch (error) {
        ctx.ui.notify(
          `graceful-reload: ${error instanceof Error ? error.message : String(error)}`,
          "error",
        );
      }
    },
  });
}
