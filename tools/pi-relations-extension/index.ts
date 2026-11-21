// This TypeScript file is executed with Bun.
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { openRelations } from "./src/sidecar.ts";
import { answer, apply, parameters } from "./src/tool.ts";

const DESCRIPTION =
  "Record how this session's goal, loop, tmux jobs, timeline tree and files relate, in this session's own Pi Durable store, and read it back.";

export default function relationsExtension(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "relations",
    label: "Relations",
    description: DESCRIPTION,
    promptSnippet:
      "Keep one graph of how this session's goal, loop, tmux jobs, timeline tree and files relate",
    promptGuidelines: [
      'Ids are "kind:ref" with kind session, goal, loop, tmux, timeline, task, doc or file, e.g. "tmux:pi-tmux-abc-1" or "doc:docs/report.md".',
      'Record a relation with action "link" and a lower-case verb ("produced", "planned", "ran", "evidenced-by"); add a label with action "add" when the ref alone is unclear.',
      'Before a final answer that spans several components, action "list" shows the whole graph; action "show" answers one node with both directions.',
    ],
    parameters,
    async execute(_toolCallId, params, _signal, _onUpdate, context: ExtensionContext) {
      const report = (error: unknown): void => {
        context.ui.notify(`Relations storage stopped: ${String(error)}`, "error");
      };
      const opened = await openRelations(context.sessionManager.getSessionFile(), report);
      try {
        const next = apply(opened.graph, params);
        if (next !== opened.graph) {
          opened.store.save(next);
          await opened.store.flush();
        }
        return {
          content: [{ type: "text", text: answer(next, params) }],
          details: { nodes: Object.keys(next.nodes).length, edges: next.edges.length },
        };
      } finally {
        await opened.close();
      }
    },
  });
}
