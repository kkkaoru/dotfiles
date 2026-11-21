import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { describe, expect, it, vi } from "vitest";
import relationsExtension from "./index.ts";
import type { OpenedRelations } from "./src/sidecar.ts";
import type { DurableRelationsStore } from "./src/store.ts";

const { notify, saved } = vi.hoisted(() => ({ notify: vi.fn(), saved: vi.fn() }));

/** A store that stops accepting writes, as a full disk looks to the extension. */
vi.mock("./src/sidecar.ts", () => ({
  openRelations: async (
    _file: string | undefined,
    report: (error: unknown) => void,
  ): Promise<OpenedRelations> => {
    const failure = new Error("disk full");
    return {
      graph: { nodes: {}, edges: [] },
      // Only the two methods the extension calls; the real store's failure path reports the same way.
      store: {
        save: (): void => {
          saved();
          report(failure);
        },
        flush: async (): Promise<void> => {
          throw failure;
        },
      } as unknown as DurableRelationsStore,
      close: async (): Promise<void> => undefined,
    };
  },
}));

function register(): { execute: (...args: unknown[]) => Promise<unknown> } {
  let tool: { execute: (...args: unknown[]) => Promise<unknown> } | undefined;
  relationsExtension({
    registerTool: (candidate: { execute: (...args: unknown[]) => Promise<unknown> }) => {
      tool = candidate;
    },
  } as unknown as ExtensionAPI);
  if (tool === undefined) {
    throw new Error("the extension registered no tool");
  }
  return tool;
}

describe("a failing relations store", () => {
  it("is reported to the user and fails the call", async () => {
    const tool = register();
    const wire = { ui: { notify }, sessionManager: { getSessionFile: (): undefined => undefined } };
    await expect(
      tool.execute(
        "call-1",
        { action: "add", id: "doc:docs/report.md" },
        undefined,
        undefined,
        wire,
      ),
    ).rejects.toThrow(/disk full/u);
    expect(saved).toHaveBeenCalled();
    expect(notify).toHaveBeenCalledWith(
      expect.stringContaining("Relations storage stopped"),
      "error",
    );
  });
});
