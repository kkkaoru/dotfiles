import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { render } from "./graph.ts";
import { edgesForCall, recordEdges, type CallFacts } from "./record.ts";
import { openRelations } from "./sidecar.ts";

const created: string[] = [];

async function remove(directory: string): Promise<void> {
  await rm(directory, { force: true, recursive: true });
}

afterEach(async () => {
  await Promise.all(created.splice(0).map(async (directory) => remove(directory)));
});

async function makeDirectory(): Promise<string> {
  const directory = await mkdtemp(path.join(tmpdir(), "relations-record-"));
  created.push(directory);
  return directory;
}

function facts(
  toolName: string,
  input: Readonly<Record<string, unknown>> = {},
  details?: unknown,
): CallFacts {
  return { sessionId: "s1", toolName, input, details };
}

function goal(id: string, objective: string): unknown {
  return { goal: { id, objective } };
}

async function graphOf(file: string): Promise<string> {
  const opened = await openRelations(file, () => undefined);
  try {
    return render(opened.graph);
  } finally {
    await opened.close();
  }
}

describe("edgesForCall and the goal", () => {
  it("records the goal a session pursued", () => {
    const edges = edgesForCall(
      facts("start_goal", { objective: "ship it" }, goal("g1", "ship it")),
    );
    expect(edges).toEqual([
      { from: "session:s1", relation: "pursued", to: "goal:g1", note: "ship it" },
    ]);
  });

  it("reads a goal that already exists", () => {
    const edges = edgesForCall(facts("get_goal", {}, goal("g1", "ship it")));
    expect(edges).toEqual([
      { from: "session:s1", relation: "pursued", to: "goal:g1", note: "ship it" },
    ]);
  });

  it("records a finished goal with its evidence", () => {
    const details = goal("g1", "x");
    const edges = edgesForCall(
      facts("update_goal", { status: "complete", reason: "passes" }, details),
    );
    expect(edges).toEqual([
      { from: "session:s1", relation: "pursued", to: "goal:g1", note: "x" },
      { from: "goal:g1", relation: "completed", to: "session:s1", note: "passes" },
    ]);
  });

  it("records a blocked goal as blocked", () => {
    const details = goal("g1", "x");
    const edges = edgesForCall(
      facts("update_goal", { status: "blocked", reason: "no key" }, details),
    );
    expect(edges[1]).toEqual({
      from: "goal:g1",
      relation: "blocked",
      to: "session:s1",
      note: "no key",
    });
  });

  it("writes nothing for a goal tool that found no goal", () => {
    expect(edgesForCall(facts("get_goal", {}, { goal: null }))).toEqual([]);
    expect(edgesForCall(facts("update_goal", { status: "complete", reason: "r" }))).toEqual([]);
  });

  it("omits a note it was not given", () => {
    const edges = edgesForCall(facts("start_goal", {}, goal("g1", "")));
    expect(edges).toEqual([{ from: "session:s1", relation: "pursued", to: "goal:g1" }]);
  });
});

describe("edgesForCall and the loop", () => {
  it("records a wakeup", () => {
    const edges = edgesForCall(facts("loop_wakeup", { reason: "check the log" }));
    expect(edges).toEqual([
      { from: "session:s1", relation: "looped", to: "loop:s1", note: "check the log" },
    ]);
  });

  it("records the completion next to the loop", () => {
    const edges = edgesForCall(facts("loop_complete", { reason: "done" }));
    expect(edges).toEqual([
      { from: "session:s1", relation: "looped", to: "loop:s1" },
      { from: "loop:s1", relation: "completed", to: "session:s1", note: "done" },
    ]);
  });

  it("clips a long note", () => {
    const edges = edgesForCall(facts("loop_wakeup", { reason: "x".repeat(400) }));
    const note: string | undefined = edges[0]?.note;
    expect(note).toHaveLength(160);
    expect(note?.endsWith("…")).toBe(true);
  });
});

describe("edgesForCall and tmux", () => {
  it("records the job a session ran", () => {
    const details = { sessionName: "pi-tmux-a-1", taskCommand: "bun test" };
    const edges = edgesForCall(facts("tmux_exec", { command: "bun test" }, details));
    expect(edges).toEqual([
      { from: "session:s1", relation: "ran", to: "tmux:pi-tmux-a-1", note: "bun test" },
    ]);
  });

  it("writes nothing for a launch that named no session", () => {
    expect(edgesForCall(facts("tmux_exec", { command: "x" }, { sessionName: 42 }))).toEqual([]);
    expect(edgesForCall(facts("tmux_exec", { command: "x" }))).toEqual([]);
  });

  it("records a check-in against its job", () => {
    const edges = edgesForCall(facts("tmux_check_in", { sessionName: "pi-tmux-a-1" }));
    expect(edges).toEqual([{ from: "session:s1", relation: "ran", to: "tmux:pi-tmux-a-1" }]);
    expect(edgesForCall(facts("tmux_check_in", { sessionName: "  " }))).toEqual([]);
  });
});

describe("edgesForCall and the timeline", () => {
  it("records the tree a session planned", () => {
    const edges = edgesForCall(facts("timeline", { action: "start", title: "the plan" }));
    expect(edges).toEqual([
      { from: "session:s1", relation: "planned", to: "timeline:tree-1", note: "the plan" },
    ]);
  });

  it("records the task it added", () => {
    const edges = edgesForCall(facts("timeline", { action: "add", id: "t1", title: "first" }));
    expect(edges).toEqual([
      { from: "session:s1", relation: "planned", to: "timeline:tree-1", note: "first" },
      { from: "timeline:tree-1", relation: "contains", to: "task:tree-1/t1", note: "first" },
    ]);
  });

  it("follows an explicitly named tree", () => {
    const edges = edgesForCall(facts("timeline", { action: "add", id: "t2", tree: "tree-3" }));
    expect(edges).toEqual([
      { from: "session:s1", relation: "planned", to: "timeline:tree-3" },
      { from: "timeline:tree-3", relation: "contains", to: "task:tree-3/t2" },
    ]);
  });

  it("ignores a tool this graph does not describe", () => {
    expect(edgesForCall(facts("bash", { command: "ls" }))).toEqual([]);
  });
});

describe("recordEdges", () => {
  it("writes the edges and leaves the graph alone on a repeat", async () => {
    const file = path.join(await makeDirectory(), "session.jsonl");
    const store = `${file}.relations-durable/main.jsonl`;
    const edges = edgesForCall(facts("loop_wakeup", { reason: "check" }));
    await recordEdges(file, edges);
    const once: string = await readFile(store, "utf8");
    await recordEdges(file, edges);
    expect(await readFile(store, "utf8")).toBe(once);
    expect(await graphOf(file)).toBe("loop:s1\nsession:s1\n  looped -> loop:s1 (check)");
  });

  it("does nothing without edges", async () => {
    const file = path.join(await makeDirectory(), "session.jsonl");
    await expect(recordEdges(file, [])).resolves.toBeUndefined();
    await expect(readFile(`${file}.relations-durable/main.jsonl`, "utf8")).rejects.toThrow();
  });

  it("stays silent when the sidecar cannot be created", async () => {
    const blocker = path.join(await makeDirectory(), "blocker");
    await writeFile(blocker, "");
    const edges = edgesForCall(facts("loop_wakeup", { reason: "check" }));
    await expect(recordEdges(path.join(blocker, "session.jsonl"), edges)).resolves.toBeUndefined();
  });
});
