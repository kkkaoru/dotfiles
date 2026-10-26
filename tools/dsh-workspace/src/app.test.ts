import { expect, test, vi } from "vitest";
import { run } from "./app.ts";
import type { Effects, FetchRequest, FetchResponse, SpawnedHost } from "./types.ts";

const REGISTRY = JSON.stringify({
  tables: { workspaces: { ws0: { path: "/work/repo", title: "repo" } } },
});

const CREATED_BODY = JSON.stringify({
  result: {
    ok: true,
    value: { created: true, workspace: { workspaceId: "ws1", path: "/work/new" } },
  },
});

const MISMATCH_BODY = JSON.stringify({
  result: {
    ok: true,
    value: { created: true, workspace: { workspaceId: "ws1", path: "/work/elsewhere" } },
  },
});

const REGISTRY_SUFFIX = "/storages/workspace.json";
const HOST_LOG = "dsh web: http://127.0.0.1:4321/?token=abc";

function fakeEffects(options: {
  readonly directoryKind: "directory" | "file" | "missing";
  readonly registryText?: string;
  readonly createResponse: FetchResponse;
}): Effects {
  const registryText = options.registryText ?? REGISTRY;
  return {
    harnessHome: () => "/home/.dsh",
    canonicalDirectory: (directory) => directory,
    directoryKind: () => options.directoryKind,
    temporaryFile: () => "/tmp/host.log",
    readText: (path) => (path.endsWith(REGISTRY_SUFFIX) ? registryText : HOST_LOG),
    removeFile: () => undefined,
    freePort: async (): Promise<number> => 4321,
    startHost: (): SpawnedHost => ({ running: () => true, stop: () => undefined }),
    fetchResponse: async (request: FetchRequest): Promise<FetchResponse> =>
      request.method === "GET"
        ? { status: 200, body: "", setCookie: "sid=abc; Path=/" }
        : options.createResponse,
    sleep: async (): Promise<void> => undefined,
  };
}

test("prints usage for --help", async () => {
  const result = await run({
    argv: ["--help"],
    cwd: "/work/repo",
    effects: fakeEffects({
      directoryKind: "directory",
      createResponse: { status: 200, body: CREATED_BODY },
    }),
  });

  expect(result.exitCode).toBe(0);
  expect(result.stdout.startsWith("usage: dsh-workspace")).toBe(true);
  expect(result.stderr).toBe("");
});

test("reports a usage error", async () => {
  const result = await run({
    argv: ["--nope"],
    cwd: "/work/repo",
    effects: fakeEffects({
      directoryKind: "directory",
      createResponse: { status: 200, body: CREATED_BODY },
    }),
  });

  expect(result).toStrictEqual({
    exitCode: 2,
    stdout: "",
    stderr: "dsh-workspace: unknown option: --nope",
  });
});

test("reports a missing directory", async () => {
  const result = await run({
    argv: ["/work/gone"],
    cwd: "/work/repo",
    effects: fakeEffects({
      directoryKind: "missing",
      createResponse: { status: 200, body: CREATED_BODY },
    }),
  });

  expect(result).toStrictEqual({
    exitCode: 1,
    stdout: "",
    stderr: "dsh-workspace: not a directory: /work/gone",
  });
});

test("reports a file instead of a directory", async () => {
  const result = await run({
    argv: ["/work/file.txt"],
    cwd: "/work/repo",
    effects: fakeEffects({
      directoryKind: "file",
      createResponse: { status: 200, body: CREATED_BODY },
    }),
  });

  expect(result.stderr).toBe("dsh-workspace: not a directory: /work/file.txt");
});

test("prints the existing workspace without a host", async () => {
  const effects = fakeEffects({
    directoryKind: "directory",
    createResponse: { status: 200, body: CREATED_BODY },
  });
  const startHost = vi.fn((): SpawnedHost => ({ running: () => true, stop: () => undefined }));
  const result = await run({ argv: [], cwd: "/work/repo", effects: { ...effects, startHost } });

  expect(result).toStrictEqual({ exitCode: 0, stdout: "exists ws0 /work/repo", stderr: "" });
  expect(startHost).not.toHaveBeenCalled();
});

test("registers a new directory", async () => {
  const result = await run({
    argv: [],
    cwd: "/work/new",
    effects: fakeEffects({
      directoryKind: "directory",
      createResponse: { status: 200, body: CREATED_BODY },
    }),
  });

  expect(result).toStrictEqual({ exitCode: 0, stdout: "created ws1 /work/new", stderr: "" });
});

test("reports a host that answered with another path", async () => {
  const result = await run({
    argv: [],
    cwd: "/work/new",
    effects: fakeEffects({
      directoryKind: "directory",
      createResponse: { status: 200, body: MISMATCH_BODY },
    }),
  });

  expect(result).toStrictEqual({
    exitCode: 1,
    stdout: "",
    stderr: "dsh-workspace: Host registered /work/elsewhere instead of /work/new",
  });
});

test("reports a host failure", async () => {
  const result = await run({
    argv: [],
    cwd: "/work/new",
    effects: fakeEffects({ directoryKind: "directory", createResponse: { status: 500, body: "" } }),
  });

  expect(result).toStrictEqual({
    exitCode: 1,
    stdout: "",
    stderr: "dsh-workspace: workspace/create request failed: HTTP 500",
  });
});

test("treats an unreadable registry as empty", async () => {
  const result = await run({
    argv: [],
    cwd: "/work/new",
    effects: fakeEffects({
      directoryKind: "directory",
      registryText: "not json",
      createResponse: { status: 200, body: CREATED_BODY },
    }),
  });

  expect(result.stdout).toBe("created ws1 /work/new");
});

test("carries the host error code through", async () => {
  const result = await run({
    argv: [],
    cwd: "/work/new",
    effects: fakeEffects({
      directoryKind: "directory",
      createResponse: {
        status: 200,
        body: JSON.stringify({
          result: { ok: false, error: { code: "gateway/boom", message: "boom" } },
        }),
      },
    }),
  });

  expect(result).toStrictEqual({
    exitCode: 1,
    stdout: "",
    stderr: "dsh-workspace: gateway/boom: boom",
  });
});

test("treats a missing registry as empty", async () => {
  const effects = fakeEffects({
    directoryKind: "directory",
    createResponse: { status: 200, body: CREATED_BODY },
  });
  const result = await run({
    argv: [],
    cwd: "/work/new",
    effects: {
      ...effects,
      readText: (file: string): string | undefined =>
        file.endsWith(REGISTRY_SUFFIX) ? undefined : HOST_LOG,
    },
  });

  expect(result.stdout).toBe("created ws1 /work/new");
});
