// This TypeScript file is executed with Bun.
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { RESUME_LINE_BUDGET, textBlock } from "./policy.ts";
import type { Runtime, SessionContextLike } from "./runtime.ts";

function entryTexts(entry: SessionEntry): readonly string[] {
  if (entry.type === "compaction" || entry.type === "branch_summary") {
    return [entry.summary];
  }
  if (entry.type !== "message") {
    return [];
  }
  const content: unknown = "content" in entry.message ? entry.message.content : undefined;
  if (!Array.isArray(content)) {
    return [];
  }
  return content.flatMap((block: unknown) => {
    const text = textBlock(block);
    return text === undefined ? [] : [text];
  });
}

function pendingLines(runtime: Runtime, entry: SessionEntry): number {
  return entryTexts(entry)
    .filter((text) => !runtime.ready.has(text) && !runtime.excluded.has(text))
    .reduce((total, text) => total + text.split("\n").length, 0);
}

/** The newest entries worth a translation request: what is on screen, newest first, until the
 *  budget runs out. A reload or a resume redraws this window, and nothing older is ever asked for.
 *  Without a session manager nothing persisted is eligible, which is the safe default. */
export function recentResumedIds(runtime: Runtime, ctx: SessionContextLike): ReadonlySet<string> {
  if (!ctx.sessionManager) {
    return new Set();
  }
  const entries = ctx.sessionManager
    .buildContextEntries()
    .filter(
      (entry) =>
        entry.type === "message" || entry.type === "compaction" || entry.type === "branch_summary",
    );
  const ids: string[] = [];
  let budget = RESUME_LINE_BUDGET;
  for (const entry of entries.toReversed()) {
    const lines = pendingLines(runtime, entry);
    // A text longer than the whole budget is left as written anyway, so it costs nothing here.
    if (budget > 0 && lines > 0 && lines <= budget) {
      budget -= lines;
      ids.push(entry.id);
    }
  }
  return new Set(ids);
}
