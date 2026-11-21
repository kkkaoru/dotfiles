import { describe, expect, it } from "vitest";
import { EMPTY, type Graph } from "./graph.ts";
import { answer, apply, type Params } from "./tool.ts";

const SESSION = "session:01a1fcbf";
const DOC = "doc:docs/report.md";

function added(): Graph {
  return apply(EMPTY, { action: "add", id: SESSION, label: "panic investigation" });
}

function linked(): Graph {
  return apply(added(), { action: "link", from: SESSION, relation: "produced", to: DOC });
}

describe("apply", () => {
  it("adds a node with its label", () => {
    expect(added().nodes[SESSION]?.label).toBe("panic investigation");
  });

  it("links two nodes, keeping the edge note", () => {
    const graph = apply(linked(), {
      action: "link",
      from: SESSION,
      relation: "ran",
      to: "tmux:pi-tmux-abc-1",
      note: "first job",
    });
    expect(graph.edges.at(-1)).toEqual({
      from: SESSION,
      relation: "ran",
      to: "tmux:pi-tmux-abc-1",
      note: "first job",
    });
  });

  it("unlinks", () => {
    const graph = apply(linked(), {
      action: "unlink",
      from: SESSION,
      relation: "produced",
      to: DOC,
    });
    expect(graph.edges).toEqual([]);
  });

  it("leaves the graph alone for the reading actions", () => {
    const graph = linked();
    expect(apply(graph, { action: "show", id: SESSION })).toBe(graph);
    expect(apply(graph, { action: "list" })).toBe(graph);
  });

  it("asks for the fields each writing action needs", () => {
    expect(() => apply(EMPTY, { action: "add" })).toThrow(/id is required/);
    expect(() => apply(EMPTY, { action: "link", from: SESSION })).toThrow(/relation is required/);
    expect(() => apply(EMPTY, { action: "link", from: SESSION, relation: "ran" })).toThrow(
      /to is required/,
    );
    expect(() => apply(EMPTY, { action: "unlink", to: DOC })).toThrow(/from is required/);
  });

  it("refuses an action it does not know", () => {
    const params: Params = JSON.parse('{"action":"rename"}');
    expect(() => apply(EMPTY, params)).toThrow(/Unknown action/);
  });
});

describe("answer", () => {
  it("answers one node for show", () => {
    expect(answer(linked(), { action: "show", id: SESSION })).toBe(
      ["session:01a1fcbf (panic investigation)", "  produced -> doc:docs/report.md"].join("\n"),
    );
  });

  it("asks for the id show needs", () => {
    expect(() => answer(linked(), { action: "show" })).toThrow(/id is required/);
  });

  it("answers the whole graph for the other actions", () => {
    expect(answer(linked(), { action: "list" })).toContain("doc:docs/report.md");
  });
});
