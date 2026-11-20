// This TypeScript file is executed with Bun.
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

/** A tool that ignores the abort keeps the run alive, and reloading then throws; give up waiting. */
export const SETTLE_TIMEOUT_MS = 3000;

function settleTimeout(): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, SETTLE_TIMEOUT_MS).unref();
  });
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Return the session to where it stood before the interrupted run: the aborted turn leaves the
 * context and its prompt goes back to the editor, so the work can run again instead of stopping
 * half-finished. Pi refuses the navigation while a tool that ignored the abort still runs.
 */
async function revert(ctx: ExtensionCommandContext, leaf: string | null): Promise<void> {
  if (leaf === null || leaf === ctx.sessionManager.getLeafId()) {
    return;
  }
  try {
    const reverted: { cancelled: boolean; editorText?: string } = await ctx.navigateTree(leaf);
    if (!reverted.cancelled && reverted.editorText !== undefined && ctx.ui.getEditorText() === "") {
      ctx.ui.setEditorText(reverted.editorText);
    }
  } catch (error) {
    ctx.ui.notify(
      `graceful-reload: could not revert the interrupted turn: ${message(error)}`,
      "warning",
    );
  }
}

export default function gracefulReload(pi: ExtensionAPI): void {
  // The leaf the live run started from. A completed run overwrites it on the next start, and a
  // reload while nothing runs never reads it.
  let startLeaf: string | null = null;

  pi.on("agent_start", (_event, ctx) => {
    startLeaf = ctx.sessionManager.getLeafId();
  });

  pi.registerCommand("graceful-reload", {
    description:
      "Reload keybindings, extensions, skills, prompts, themes, and context files, reverting a running response instead of waiting for it",
    async handler(_args, ctx) {
      // The built-in /reload refuses while a response or compaction is live. Aborting and reverting
      // keeps the session at its pre-run state, so the interrupted turn can run again.
      if (!ctx.isIdle()) {
        ctx.abort();
        await Promise.race([ctx.waitForIdle(), settleTimeout()]);
        if (ctx.isIdle()) {
          await revert(ctx, startLeaf);
        }
      }
      startLeaf = null;
      try {
        await ctx.reload();
      } catch (error) {
        ctx.ui.notify(`graceful-reload: ${message(error)}`, "error");
      }
    },
  });
}
