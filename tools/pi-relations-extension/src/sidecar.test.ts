import { mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { addNode, EMPTY, link, type Graph } from "./graph.ts";
import { openRelations, SIDECAR } from "./sidecar.ts";
import { DurableRelationsStore } from "./store.ts";

const SESSION = "session:01a1fcbf";
const DOC = "doc:docs/report.md";

function graph(): Graph {
  return link(addNode(EMPTY, SESSION, "panic investigation"), {
    from: SESSION,
    relation: "produced",
    to: DOC,
  });
}

const directories: string[] = [];

async function sessionFile(): Promise<string> {
  const directory = await mkdtemp(path.join(tmpdir(), "relations-"));
  directories.push(directory);
  return path.join(directory, "session.jsonl");
}

afterEach(async () => {
  await Promise.all(
    directories.splice(0).map(async (directory) => {
      await rm(directory, { recursive: true });
    }),
  );
});

describe("openRelations", () => {
  it("keeps an in-memory graph when the session has no file", async () => {
    const opened = await openRelations(undefined, vi.fn());
    expect(opened.graph).toEqual(EMPTY);
    opened.store.save(graph());
    await opened.store.flush();
    await opened.close();
  });

  it("keeps the graph beside the session file", async () => {
    const file = await sessionFile();
    const first = await openRelations(file, vi.fn());
    first.store.save(graph());
    await first.store.flush();
    await first.close();

    expect(await readdir(`${file}${SIDECAR}`)).toContainEqual(expect.stringMatching(/\.jsonl$/));

    const second = await openRelations(file, vi.fn());
    expect(second.graph).toEqual(graph());
    await second.close();
  });

  it("creates the sidecar directory", async () => {
    const file = await sessionFile();
    const opened = await openRelations(file, vi.fn());
    await opened.close();
    expect(await readdir(path.dirname(file))).toContain(`session.jsonl${SIDECAR}`);
  });

  it("refuses a sidecar path that is already a file", async () => {
    const file = await sessionFile();
    await writeFile(`${file}${SIDECAR}`, "not a directory");
    await expect(openRelations(file, vi.fn())).rejects.toThrow();
  });

  it("releases the lock when the store cannot open", async () => {
    const file = await sessionFile();
    vi.spyOn(DurableRelationsStore, "open").mockRejectedValueOnce(new Error("boom"));
    await expect(openRelations(file, vi.fn())).rejects.toThrow(/boom/);
    // The lock was dropped, so the next open succeeds.
    const next = await openRelations(file, vi.fn());
    await expect(next.close()).resolves.toBeUndefined();
  });
});
