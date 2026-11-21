// This TypeScript file is executed with Bun.

/** What a relation endpoint can be: a durable concept of this harness, or a file on disk. */
export const KINDS = [
  "session",
  "goal",
  "loop",
  "tmux",
  "timeline",
  "task",
  "doc",
  "file",
] as const;
export type Kind = (typeof KINDS)[number];

export interface Node {
  readonly kind: Kind;
  readonly ref: string;
  readonly label?: string;
}

export interface Edge {
  readonly from: string;
  readonly relation: string;
  readonly to: string;
  readonly note?: string;
}

/** Nodes keyed by their id (`kind:ref`), plus the directed edges between them. */
export interface Graph {
  readonly nodes: Readonly<Record<string, Node>>;
  readonly edges: readonly Edge[];
}

export const EMPTY: Graph = { nodes: {}, edges: [] };

const RELATION = /^[a-z][a-z0-9-]*$/u;

function isKind(value: string): value is Kind {
  return (KINDS as readonly string[]).includes(value);
}

/** The node an id names, or undefined when it is not `kind:ref` with a known kind. */
export function parseId(id: string): Node | undefined {
  const trimmed = id.trim();
  const colon = trimmed.indexOf(":");
  if (colon <= 0) {
    return undefined;
  }
  const kind = trimmed.slice(0, colon);
  const ref = trimmed.slice(colon + 1).trim();
  if (ref === "" || !isKind(kind)) {
    return undefined;
  }
  return { kind, ref };
}

export function idOf(node: Node): string {
  return `${node.kind}:${node.ref}`;
}

function requireNode(id: string): Node {
  const node = parseId(id);
  if (node === undefined) {
    throw new Error(`"${id}" is not a node id; use one of ${KINDS.join(", ")} followed by ":".`);
  }
  return node;
}

export function requireId(id: string): string {
  return idOf(requireNode(id));
}

function requireRelation(relation: string): string {
  const trimmed = relation.trim();
  if (!RELATION.test(trimmed)) {
    throw new Error(`"${relation}" is not a relation; use lower-case words joined by "-".`);
  }
  return trimmed;
}

function cleanNote(note: string | undefined): string | undefined {
  const trimmed = note?.trim();
  return trimmed === undefined || trimmed === "" ? undefined : trimmed;
}

function withNote(note: string | undefined): { note?: string } {
  return note === undefined ? {} : { note };
}

/** Add or relabel a node. Adding an existing node again changes nothing. */
export function addNode(graph: Graph, id: string, label?: string): Graph {
  const node = requireNode(id);
  const key = idOf(node);
  const prior = graph.nodes[key];
  const next = cleanNote(label);
  if (prior !== undefined && (next === undefined || prior.label === next)) {
    return graph;
  }
  const merged: Node = next === undefined ? node : { ...node, label: next };
  return { nodes: { ...graph.nodes, [key]: merged }, edges: graph.edges };
}

/** Link two nodes, creating either one as a bare node when it is not recorded yet. */
export function link(graph: Graph, edge: Edge): Graph {
  const from = requireId(edge.from);
  const to = requireId(edge.to);
  const relation = requireRelation(edge.relation);
  const note = cleanNote(edge.note);
  const same = (candidate: Edge): boolean =>
    candidate.from === from && candidate.relation === relation && candidate.to === to;
  const prior = graph.edges.find(same);
  if (prior !== undefined && prior.note === note) {
    return graph;
  }
  const { nodes } = addNode(addNode(graph, from), to);
  const edges = [
    ...graph.edges.filter((candidate) => !same(candidate)),
    { from, relation, to, ...withNote(note) },
  ];
  return { nodes, edges };
}

export function unlink(graph: Graph, edge: Edge): Graph {
  const from = requireId(edge.from);
  const to = requireId(edge.to);
  const relation = requireRelation(edge.relation);
  const edges = graph.edges.filter(
    (candidate) =>
      !(candidate.from === from && candidate.relation === relation && candidate.to === to),
  );
  return edges.length === graph.edges.length ? graph : { nodes: graph.nodes, edges };
}

function noteOf(edge: Edge): string {
  return edge.note === undefined ? "" : ` (${edge.note})`;
}

function nodeLines(graph: Graph, id: string, both: boolean): string[] {
  const node = graph.nodes[id];
  if (node === undefined) {
    throw new Error(`No node ${id}.`);
  }
  const head = node.label === undefined ? id : `${id} (${node.label})`;
  const out = graph.edges
    .filter((edge) => edge.from === id)
    .map((edge) => `  ${edge.relation} -> ${edge.to}${noteOf(edge)}`);
  const into = both
    ? graph.edges
        .filter((edge) => edge.to === id)
        .map((edge) => `  <- ${edge.relation} ${edge.from}${noteOf(edge)}`)
    : [];
  return [head, ...out, ...into];
}

/** The whole graph as text, or one node with both directions of its edges. */
export function render(graph: Graph, only?: string): string {
  if (only !== undefined) {
    return nodeLines(graph, requireId(only), true).join("\n");
  }
  const ids = Object.keys(graph.nodes).toSorted();
  const lines = ids.flatMap((id) => nodeLines(graph, id, false));
  return lines.length === 0 ? "no relations" : lines.join("\n");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function graphNode(key: string, value: unknown): Node {
  if (!isRecord(value)) {
    throw new TypeError(`Invalid stored node ${key}.`);
  }
  const { ref, label } = value;
  if (typeof ref !== "string") {
    throw new TypeError(`Invalid stored node ${key}.`);
  }
  const node = requireNode(key);
  if (node.ref !== ref) {
    throw new TypeError(`Stored node ${key} does not match its key.`);
  }
  if (label !== undefined && typeof label !== "string") {
    throw new TypeError(`Invalid label on ${key}.`);
  }
  return label === undefined ? node : { ...node, label };
}

function graphEdge(value: unknown): Edge {
  if (!isRecord(value)) {
    throw new TypeError("Invalid stored edge.");
  }
  const { from, relation, to, note } = value;
  if (typeof from !== "string" || typeof relation !== "string" || typeof to !== "string") {
    throw new TypeError("Invalid stored edge.");
  }
  if (note !== undefined && typeof note !== "string") {
    throw new TypeError("Invalid stored edge note.");
  }
  return { from, relation, to, ...withNote(note) };
}

/** Read a graph back from its stored JSON, refusing anything that is not one. */
export function parseGraph(value: unknown): Graph {
  if (!isRecord(value)) {
    throw new TypeError("Invalid stored relations.");
  }
  const { nodes: stored, edges: storedEdges } = value;
  if (!isRecord(stored) || !Array.isArray(storedEdges)) {
    throw new TypeError("Invalid stored relations.");
  }
  const nodes: Record<string, Node> = {};
  for (const [key, node] of Object.entries(stored)) {
    nodes[key] = graphNode(key, node);
  }
  return { nodes, edges: storedEdges.map((edge: unknown) => graphEdge(edge)) };
}
