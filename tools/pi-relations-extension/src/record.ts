// This TypeScript file is executed with Bun.
import { link, type Edge, type Graph } from "./graph.ts";
import { openRelations } from "./sidecar.ts";

/** What a finished tool call tells the recorder. */
export interface CallFacts {
  readonly sessionId: string;
  readonly toolName: string;
  readonly input: Readonly<Record<string, unknown>>;
  readonly details: unknown;
}

/** A note is a hint, not a copy of the record: long objectives would bloat every commit. */
const NOTE_LIMIT = 160;

/** The tree a timeline call touches; the extension starts at tree-1 and a call may name another. */
const FIRST_TREE = "tree-1";

function text(value: unknown): string | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  const trimmed = value.trim();
  return trimmed === "" ? undefined : trimmed;
}

function clipped(value: unknown): string | undefined {
  const trimmed = text(value);
  if (trimmed === undefined || trimmed.length <= NOTE_LIMIT) {
    return trimmed;
  }
  return `${trimmed.slice(0, NOTE_LIMIT - 1)}…`;
}

function field(source: unknown, key: string): unknown {
  if (typeof source !== "object" || source === null) {
    return undefined;
  }
  return (source as Record<string, unknown>)[key];
}

function edge(from: string, relation: string, to: string, note?: string): Edge {
  return note === undefined ? { from, relation, to } : { from, relation, to, note };
}

function sessionOf(facts: CallFacts): string {
  return `session:${facts.sessionId}`;
}

/** The goal this call is about; a goal tool that found none yields no id, so nothing is recorded. */
function goalOf(facts: CallFacts): string | undefined {
  const id = text(field(field(facts.details, "goal"), "id"));
  return id === undefined ? undefined : `goal:${id}`;
}

function objectiveOf(facts: CallFacts): string | undefined {
  return clipped(field(field(facts.details, "goal"), "objective"));
}

function pursuit(facts: CallFacts): readonly Edge[] {
  const goal = goalOf(facts);
  return goal === undefined ? [] : [edge(sessionOf(facts), "pursued", goal, objectiveOf(facts))];
}

function goalStatus(facts: CallFacts): readonly Edge[] {
  const goal = goalOf(facts);
  if (goal === undefined) {
    return [];
  }
  const relation: string = facts.input["status"] === "complete" ? "completed" : "blocked";
  return [
    edge(sessionOf(facts), "pursued", goal, objectiveOf(facts)),
    edge(goal, relation, sessionOf(facts), clipped(facts.input["reason"])),
  ];
}

/** One loop per session, so the session id names it. */
function loopOf(facts: CallFacts): string {
  return `loop:${facts.sessionId}`;
}

function loopStarted(facts: CallFacts): readonly Edge[] {
  return [edge(sessionOf(facts), "looped", loopOf(facts), clipped(facts.input["reason"]))];
}

function loopFinished(facts: CallFacts): readonly Edge[] {
  const session = sessionOf(facts);
  return [
    edge(session, "looped", loopOf(facts)),
    edge(loopOf(facts), "completed", session, clipped(facts.input["reason"])),
  ];
}

function tmuxLaunch(facts: CallFacts): readonly Edge[] {
  const name = text(field(facts.details, "sessionName"));
  if (name === undefined) {
    return [];
  }
  const command = clipped(field(facts.details, "taskCommand"));
  return [edge(sessionOf(facts), "ran", `tmux:${name}`, command)];
}

function tmuxCheckIn(facts: CallFacts): readonly Edge[] {
  const name = text(facts.input["sessionName"]);
  return name === undefined ? [] : [edge(sessionOf(facts), "ran", `tmux:${name}`)];
}

function timelineOf(facts: CallFacts): string {
  return `timeline:${text(facts.input["tree"]) ?? FIRST_TREE}`;
}

function plan(facts: CallFacts): readonly Edge[] {
  const timeline = timelineOf(facts);
  const title = clipped(facts.input["title"]);
  const planned = edge(sessionOf(facts), "planned", timeline, title);
  const id = text(facts.input["id"]);
  if (facts.input["action"] !== "add" || id === undefined) {
    return [planned];
  }
  const tree = timeline.slice("timeline:".length);
  return [planned, edge(timeline, "contains", `task:${tree}/${id}`, title)];
}

/**
 * The edges one tool call implies, in the vocabulary this graph uses: session pursued goal,
 * goal completed or blocked session, session looped loop, loop completed session, session ran
 * tmux, session planned timeline, timeline contains task.
 *
 * Nothing is inferred beyond the call: a goal that does not exist yet has no id, and a failed
 * tmux launch has no session to name.
 */
export function edgesForCall(facts: CallFacts): readonly Edge[] {
  switch (facts.toolName) {
    case "start_goal":
    case "get_goal":
    case "goal_wait": {
      return pursuit(facts);
    }
    case "update_goal": {
      return goalStatus(facts);
    }
    case "loop_wakeup": {
      return loopStarted(facts);
    }
    case "loop_complete": {
      return loopFinished(facts);
    }
    case "tmux_exec": {
      return tmuxLaunch(facts);
    }
    case "tmux_check_in": {
      return tmuxCheckIn(facts);
    }
    case "timeline": {
      return plan(facts);
    }
    default: {
      return [];
    }
  }
}

/** A recorded edge is bookkeeping, so a storage failure is dropped rather than reported here. */
function quiet(): void {
  // Intentionally silent: the relations tool reports storage errors when it is asked to write.
}

/**
 * Record these edges into the session's graph. Best effort: a relation is bookkeeping, so a
 * lock held elsewhere or a full disk is dropped rather than failing the tool call that caused it.
 */
export async function recordEdges(file: string | undefined, edges: readonly Edge[]): Promise<void> {
  if (edges.length === 0) {
    return;
  }
  try {
    const opened = await openRelations(file, quiet);
    try {
      let graph: Graph = opened.graph;
      for (const candidate of edges) {
        graph = link(graph, candidate);
      }
      if (graph !== opened.graph) {
        opened.store.save(graph);
        await opened.store.flush();
      }
    } finally {
      await opened.close();
    }
  } catch {
    quiet();
  }
}
