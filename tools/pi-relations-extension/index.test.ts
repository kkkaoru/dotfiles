import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { describe, expect, it, vi } from "vitest";
import relationsExtension from "./index.ts";

const SESSION = "session:01a1fcbf";
const DOC = "doc:docs/report.md";

interface Registered {
  name: string;
  execute: (
    id: string,
    params: Record<string, unknown>,
    signal: unknown,
    update: unknown,
    context: unknown,
  ) => Promise<{ content: { text: string }[]; details: unknown }>;
}

function register(): Registered {
  let tool: Registered | undefined;
  const pi = {
    registerTool: (candidate: Registered) => {
      tool = candidate;
    },
  } as unknown as ExtensionAPI;
  relationsExtension(pi);
  if (tool === undefined) {
    throw new Error("the extension registered no tool");
  }
  return tool;
}

/** A session without a file keeps the graph in memory, which is all this wiring test needs. */
function context(): unknown {
  return {
    ui: { notify: vi.fn() },
    sessionManager: { getSessionFile: (): undefined => undefined },
  };
}

describe("relationsExtension", () => {
  it("registers the relations tool", () => {
    expect(register().name).toBe("relations");
  });

  it("answers a write with the new graph", async () => {
    const tool = register();
    const result = await tool.execute(
      "call-1",
      { action: "link", from: SESSION, relation: "produced", to: DOC },
      undefined,
      undefined,
      context(),
    );
    expect(result.content[0]?.text).toContain(`produced -> ${DOC}`);
    expect(result.details).toEqual({ nodes: 2, edges: 1 });
  });

  it("answers a read without writing", async () => {
    const tool = register();
    const result = await tool.execute(
      "call-2",
      { action: "list" },
      undefined,
      undefined,
      context(),
    );
    expect(result.content[0]?.text).toBe("no relations");
    expect(result.details).toEqual({ nodes: 0, edges: 0 });
  });

  it("reports a bad id and leaves no store open", async () => {
    const tool = register();
    await expect(
      tool.execute("call-3", { action: "add", id: "widget:x" }, undefined, undefined, context()),
    ).rejects.toThrow(/not a node id/u);
  });
});
