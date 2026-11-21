import { describe, expect, it } from "vitest";
import {
  addNode,
  EMPTY,
  idOf,
  link,
  parseGraph,
  parseId,
  render,
  requireId,
  unlink,
  type Edge,
  type Graph,
} from "./graph.ts";

const SESSION = "session:01a1fcbf";
const DOC = "doc:docs/report.md";

function withSession(): Graph {
  return addNode(EMPTY, SESSION, "panic investigation");
}

describe("parseId", () => {
  it("splits a known kind from its ref", () => {
    expect(parseId("tmux:pi-tmux-abc-1")).toEqual({ kind: "tmux", ref: "pi-tmux-abc-1" });
  });

  it("trims both sides", () => {
    expect(parseId("  doc: docs/a.md ")).toEqual({ kind: "doc", ref: "docs/a.md" });
  });

  it("refuses an unknown kind, a missing ref and a missing colon", () => {
    expect(parseId("widget:x")).toBeUndefined();
    expect(parseId("doc:")).toBeUndefined();
    expect(parseId("docs/a.md")).toBeUndefined();
    expect(parseId(":ref")).toBeUndefined();
  });

  it("keeps a colon inside the ref", () => {
    expect(parseId("doc:a:b.md")).toEqual({ kind: "doc", ref: "a:b.md" });
  });
});

describe("addNode", () => {
  it("adds a labelled node", () => {
    const graph = withSession();
    expect(graph.nodes[SESSION]).toEqual({
      kind: "session",
      ref: "01a1fcbf",
      label: "panic investigation",
    });
  });

  it("returns the same graph when nothing changes", () => {
    const graph = withSession();
    expect(addNode(graph, SESSION, "panic investigation")).toBe(graph);
    expect(addNode(graph, SESSION)).toBe(graph);
  });

  it("relabels an existing node", () => {
    expect(addNode(withSession(), SESSION, "other").nodes[SESSION]?.label).toBe("other");
  });

  it("normalises the id it is given", () => {
    expect(addNode(EMPTY, " doc: docs/a.md ").nodes["doc:docs/a.md"]).toBeDefined();
  });

  it("refuses a bad id", () => {
    expect(() => addNode(EMPTY, "widget:x")).toThrow(/not a node id/);
  });

  it("ignores a blank label", () => {
    expect(addNode(EMPTY, DOC, "  ").nodes[DOC]?.label).toBeUndefined();
  });
});

describe("link", () => {
  const edge: Edge = { from: SESSION, relation: "produced", to: DOC, note: "the report" };

  it("creates both endpoints and the edge", () => {
    const graph = link(withSession(), edge);
    expect(Object.keys(graph.nodes).toSorted()).toEqual([DOC, SESSION].toSorted());
    expect(graph.edges).toEqual([edge]);
  });

  it("keeps the label of an endpoint it already knows", () => {
    expect(link(withSession(), edge).nodes[SESSION]?.label).toBe("panic investigation");
  });

  it("is idempotent and updates the note", () => {
    const once = link(withSession(), edge);
    expect(link(once, edge)).toBe(once);
    expect(link(once, { ...edge, note: "newer" }).edges[0]?.note).toBe("newer");
  });

  it("drops a blank note", () => {
    expect(link(withSession(), { ...edge, note: " " }).edges[0]).toEqual({
      from: SESSION,
      relation: "produced",
      to: DOC,
    });
  });

  it("refuses a relation that is not a lower-case word", () => {
    expect(() => link(withSession(), { ...edge, relation: "Produced By" })).toThrow(
      /not a relation/,
    );
  });

  it("refuses a bad endpoint", () => {
    expect(() => link(withSession(), { ...edge, to: "nope" })).toThrow(/not a node id/);
  });
});

describe("unlink", () => {
  const edge: Edge = { from: SESSION, relation: "produced", to: DOC };

  it("removes the edge and keeps the nodes", () => {
    const graph = unlink(link(withSession(), edge), edge);
    expect(graph.edges).toEqual([]);
    expect(Object.keys(graph.nodes)).toContain(DOC);
  });

  it("returns the same graph when the edge is absent", () => {
    const graph = link(withSession(), edge);
    expect(unlink(graph, { ...edge, relation: "ran" })).toBe(graph);
  });

  it("refuses a bad relation", () => {
    expect(() => unlink(withSession(), { ...edge, relation: "Nope" })).toThrow(/not a relation/);
  });
});

describe("render", () => {
  const graph = link(link(withSession(), { from: SESSION, relation: "produced", to: DOC }), {
    from: SESSION,
    relation: "planned",
    to: "timeline:relations",
  });

  it("says so when there is nothing", () => {
    expect(render(EMPTY)).toBe("no relations");
  });

  it("lists every node with its outgoing edges", () => {
    expect(render(graph)).toBe(
      [
        DOC,
        "session:01a1fcbf (panic investigation)",
        "  produced -> doc:docs/report.md",
        "  planned -> timeline:relations",
        "timeline:relations",
      ].join("\n"),
    );
  });

  it("shows one node with both directions", () => {
    expect(render(graph, DOC)).toBe(
      ["doc:docs/report.md", "  <- produced session:01a1fcbf"].join("\n"),
    );
  });

  it("normalises the id it is asked about", () => {
    expect(render(graph, ` ${DOC} `)).toContain("doc:docs/report.md");
  });

  it("refuses an unknown node", () => {
    expect(() => render(graph, "doc:docs/other.md")).toThrow(/No node/);
  });

  it("prints the note beside the edge", () => {
    const noted = link(EMPTY, { from: SESSION, relation: "produced", to: DOC, note: "why" });
    expect(render(noted, DOC)).toContain("(why)");
  });
});

describe("parseGraph", () => {
  const graph = link(withSession(), { from: SESSION, relation: "produced", to: DOC, note: "n" });

  it("round-trips a stored graph", () => {
    expect(parseGraph(structuredClone(graph))).toEqual(graph);
  });

  it("refuses something that is not a graph", () => {
    expect(() => parseGraph(null)).toThrow(/Invalid stored relations/);
    expect(() => parseGraph({ nodes: {}, edges: {} })).toThrow(/Invalid stored relations/);
    expect(() => parseGraph({ nodes: { "doc:a": { ref: "a" } }, edges: [1] })).toThrow(
      /Invalid stored edge/,
    );
  });

  it("refuses a node whose key and ref disagree", () => {
    expect(() => parseGraph({ nodes: { "doc:a": { kind: "doc", ref: "b" } }, edges: [] })).toThrow(
      /does not match its key/,
    );
  });

  it("refuses a node with a bad label and an edge with a bad note", () => {
    expect(() => parseGraph({ nodes: { "doc:a": { ref: "a", label: 1 } }, edges: [] })).toThrow(
      /Invalid label/,
    );
    expect(() =>
      parseGraph({ nodes: {}, edges: [{ from: "doc:a", relation: "ran", to: "doc:b", note: 1 }] }),
    ).toThrow(/Invalid stored edge note/);
  });

  it("refuses an unknown node kind and a missing ref", () => {
    expect(() => parseGraph({ nodes: { "widget:x": { ref: "x" } }, edges: [] })).toThrow(
      /not a node id/,
    );
    expect(() => parseGraph({ nodes: { "doc:x": {} }, edges: [] })).toThrow(/Invalid stored node/);
  });
});

describe("idOf and requireId", () => {
  it("builds an id back from a node", () => {
    expect(idOf({ kind: "task", ref: "t1" })).toBe("task:t1");
  });

  it("refuses a bad id", () => {
    expect(() => requireId("widget:x")).toThrow(/not a node id/);
  });
});
