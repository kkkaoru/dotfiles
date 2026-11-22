import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
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

type ToolResultHandler = (event: Record<string, unknown>, context: unknown) => Promise<void> | void;

function register(): { handlers: Map<string, ToolResultHandler>; tool: Registered } {
  let tool: Registered | undefined;
  const handlers = new Map<string, ToolResultHandler>();
  const pi = {
    on: (name: string, handler: ToolResultHandler) => {
      handlers.set(name, handler);
    },
    registerTool: (candidate: Registered) => {
      tool = candidate;
    },
  } as unknown as ExtensionAPI;
  relationsExtension(pi);
  if (tool === undefined) {
    throw new Error("the extension registered no tool");
  }
  return { handlers, tool };
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
    expect(register().tool.name).toBe("relations");
  });

  it("watches every finished tool call", () => {
    expect([...register().handlers.keys()]).toEqual(["tool_result"]);
  });

  it("records the edges of a finished tool call", async () => {
    const { handlers, tool } = register();
    const handler = handlers.get("tool_result");
    if (handler === undefined) {
      throw new Error("the extension registered no tool_result handler");
    }
    const directory = await mkdtemp(path.join(tmpdir(), "relations-index-"));
    const file = path.join(directory, "session.jsonl");
    const wire = {
      sessionManager: { getSessionFile: (): string => file, getSessionId: (): string => "s1" },
      ui: { notify: vi.fn() },
    };
    try {
      await handler({ input: {}, toolName: "bash" }, wire);
      const empty = await tool.execute("call-4", { action: "list" }, undefined, undefined, wire);
      expect(empty.content[0]?.text).toBe("no relations");
      await handler({ input: { reason: "check" }, toolName: "loop_wakeup" }, wire);
      const listed = await tool.execute("call-5", { action: "list" }, undefined, undefined, wire);
      expect(listed.content[0]?.text).toContain("looped -> loop:s1");
    } finally {
      await rm(directory, { force: true, recursive: true });
    }
  });

  it("answers a write with the new graph", async () => {
    const { tool } = register();
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
    const { tool } = register();
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
    const { tool } = register();
    await expect(
      tool.execute("call-3", { action: "add", id: "widget:x" }, undefined, undefined, context()),
    ).rejects.toThrow(/not a node id/u);
  });
});
