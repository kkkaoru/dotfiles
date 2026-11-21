// This TypeScript file is executed with Bun.
import { StringEnum } from "@earendil-works/pi-ai";
import { Type, type Static } from "typebox";
import { addNode, KINDS, link, render, unlink, type Graph } from "./graph.ts";

export const ACTIONS = ["add", "link", "unlink", "show", "list"] as const;
export type Action = (typeof ACTIONS)[number];

const idField = Type.Optional(
  Type.String({ description: `Node id: kind:ref, where kind is one of ${KINDS.join(", ")}` }),
);

export const parameters = Type.Object({
  action: StringEnum(ACTIONS),
  id: idField,
  label: Type.Optional(Type.String({ description: "Human label for the node, e.g. its title" })),
  from: Type.Optional(Type.String({ description: "Node the relation starts at" })),
  relation: Type.Optional(
    Type.String({ description: 'Relation name in lower case, e.g. "produced", "planned", "ran"' }),
  ),
  to: Type.Optional(Type.String({ description: "Node the relation points at" })),
  note: Type.Optional(Type.String({ description: "Optional detail kept with the edge" })),
});

export type Params = Static<typeof parameters>;

function need(value: string | undefined, name: string): string {
  if (value === undefined || value.trim() === "") {
    throw new Error(`${name} is required for this action.`);
  }
  return value;
}

function edgeOf(params: Params): { from: string; relation: string; to: string; note?: string } {
  const from = need(params.from, "from");
  const relation = need(params.relation, "relation");
  const to = need(params.to, "to");
  return params.note === undefined
    ? { from, relation, to }
    : { from, relation, to, note: params.note };
}

/** The graph after this call. Reading actions return it unchanged. */
export function apply(graph: Graph, params: Params): Graph {
  switch (params.action) {
    case "add": {
      return addNode(graph, need(params.id, "id"), params.label);
    }
    case "link": {
      return link(graph, edgeOf(params));
    }
    case "unlink": {
      return unlink(graph, edgeOf(params));
    }
    case "show":
    case "list": {
      return graph;
    }
    default: {
      throw new Error(`Unknown action ${String(params.action)}.`);
    }
  }
}

/** The text this call answers with. */
export function answer(graph: Graph, params: Params): string {
  return params.action === "show" ? render(graph, need(params.id, "id")) : render(graph);
}
