// This TypeScript file is executed with Bun.
import type {
  ExtensionAPI,
  ExtensionCommandContext,
  SessionEntry,
} from "@earendil-works/pi-coding-agent";

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

/** pi returns a target's text to the editor only when the target is a prompt the user typed. */
function isPrompt(entry: SessionEntry): boolean {
  return entry.type === "message" && entry.message.role === "user";
}

/**
 * The first entry the interrupted run appended, or undefined when this branch no longer descends
 * from where the run started. pi emits agent_start before it persists the prompt, so `start` is the
 * leaf just before that prompt.
 */
function runStart(
  manager: ExtensionCommandContext["sessionManager"],
  start: string | null,
): SessionEntry | undefined {
  let id = manager.getLeafId();
  let first: SessionEntry | undefined;
  while (id !== null && id !== start) {
    const entry = manager.getEntry(id);
    if (entry === undefined) {
      return undefined;
    }
    first = entry;
    id = entry.parentId;
  }
  return id === start ? first : undefined;
}

/**
 * Return the session to where it stood before the interrupted run, with that run's prompt back in
 * the editor so the work can run again. Navigating to the prompt itself is what returns its text;
 * navigating to the leaf the run started from would drop the prompt along with the partial answer.
 */
async function revert(ctx: ExtensionCommandContext, start: string | null): Promise<void> {
  const current = ctx.sessionManager.getLeafId();
  if (current === null || current === start) {
    return;
  }
  const first = runStart(ctx.sessionManager, start);
  if (first === undefined) {
    // The branch moved away from where this run started, so navigating would drop untold work.
    return;
  }
  const target = isPrompt(first) ? first.id : start;
  if (target === null) {
    return;
  }
  try {
    const reverted: { cancelled: boolean; editorText?: string } = await ctx.navigateTree(target);
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
  // The leaf the live run started from, which is the one before its prompt. A completed run
  // overwrites it on the next start, and a reload while nothing runs never reads it.
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
