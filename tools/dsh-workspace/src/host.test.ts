import { expect, test, vi } from "vitest";
import {
  createRequestBody,
  createWorkspace,
  findHostUrl,
  hostOrigin,
  parseCreateResponse,
  sessionCookie,
} from "./host.ts";
import type { Effects, FetchRequest, FetchResponse, SpawnedHost } from "./types.ts";

const SUCCESS_BODY = JSON.stringify({
  result: {
    ok: true,
    value: { created: true, workspace: { workspaceId: "ws1", path: "/work/repo" } },
  },
});

const EXISTING_BODY = JSON.stringify({
  result: {
    ok: true,
    value: { created: false, workspace: { workspaceId: "ws1", path: "/work/repo" } },
  },
});

const ERROR_BODY = JSON.stringify({
  result: { ok: false, error: { code: "gateway/arguments-invalid", message: "bad request" } },
});

function fakeEffects(options: {
  readonly logText: string;
  readonly running: boolean;
  readonly createResponse: FetchResponse;
  readonly authResponse: FetchResponse;
}): Effects & {
  readonly stop: ReturnType<typeof vi.fn>;
  readonly remove: ReturnType<typeof vi.fn>;
  readonly calls: FetchRequest[];
} {
  const calls: FetchRequest[] = [];
  const stop = vi.fn();
  const remove = vi.fn();
  return {
    calls,
    stop,
    remove,
    harnessHome: () => "/home/.dsh",
    canonicalDirectory: (directory) => directory,
    directoryKind: () => "directory",
    temporaryFile: () => "/tmp/host.log",
    readText: () => options.logText,
    removeFile: remove,
    freePort: async (): Promise<number> => 4321,
    startHost: (): SpawnedHost => ({ running: () => options.running, stop }),
    fetchResponse: async (request: FetchRequest): Promise<FetchResponse> => {
      calls.push(request);
      return request.method === "GET" ? options.authResponse : options.createResponse;
    },
    sleep: async (): Promise<void> => undefined,
  };
}

test("finds the printed host url", () => {
  expect(findHostUrl("noise\ndsh web: http://127.0.0.1:4321/?token=abc_DEF-1\n", 4321)).toBe(
    "http://127.0.0.1:4321/?token=abc_DEF-1",
  );
});

test("ignores a url printed for another port", () => {
  expect(findHostUrl("http://127.0.0.1:9999/?token=abc", 4321)).toBeUndefined();
});

test("keeps only the scheme and authority", () => {
  expect(hostOrigin("http://127.0.0.1:4321/?token=abc")).toBe("http://127.0.0.1:4321");
});

test("returns the input when it is not a url", () => {
  expect(hostOrigin("nope")).toBe("nope");
});

test("takes the first cookie pair", () => {
  expect(sessionCookie("sid=abc; Path=/; HttpOnly")).toBe("sid=abc");
});

test("returns undefined without a set-cookie header", () => {
  expect(sessionCookie(undefined)).toBeUndefined();
});

test("builds the request envelope the gateway expects", () => {
  expect(JSON.parse(createRequestBody("/work/repo", "repo"))).toStrictEqual({
    type: "client-request",
    rpcId: "dsh-workspace",
    method: "workspace/create",
    payload: { args: { request: { path: "/work/repo", title: "repo" } } },
  });
});

test("reads a created workspace", () => {
  expect(parseCreateResponse({ status: 200, body: SUCCESS_BODY })).toStrictEqual({
    kind: "created",
    workspace: { workspaceId: "ws1", path: "/work/repo" },
  });
});

test("reads an already registered workspace", () => {
  expect(parseCreateResponse({ status: 200, body: EXISTING_BODY })).toStrictEqual({
    kind: "exists",
    workspace: { workspaceId: "ws1", path: "/work/repo" },
  });
});

test("reports an http failure", () => {
  expect(parseCreateResponse({ status: 500, body: "" })).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed: HTTP 500",
  });
});

test("reports unparsable json", () => {
  expect(parseCreateResponse({ status: 200, body: "<html>" })).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed",
  });
});

test("reports an envelope without result", () => {
  expect(parseCreateResponse({ status: 200, body: JSON.stringify({ result: 4 }) })).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed",
  });
});

test("reports the host error code and message", () => {
  expect(parseCreateResponse({ status: 200, body: ERROR_BODY })).toStrictEqual({
    kind: "error",
    message: "gateway/arguments-invalid: bad request",
  });
});

test("falls back when the host error carries no fields", () => {
  expect(
    parseCreateResponse({
      status: 200,
      body: JSON.stringify({ result: { ok: false, error: {} } }),
    }),
  ).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed: workspace/create request failed",
  });
});

test("falls back when the host error is not an object", () => {
  expect(
    parseCreateResponse({ status: 200, body: JSON.stringify({ result: { ok: false } }) }),
  ).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed",
  });
});

test("reports a value without a workspace", () => {
  expect(
    parseCreateResponse({ status: 200, body: JSON.stringify({ result: { ok: true, value: {} } }) }),
  ).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed",
  });
});

test("reports a workspace without string fields", () => {
  const body = JSON.stringify({
    result: { ok: true, value: { created: true, workspace: { path: 7 } } },
  });
  expect(parseCreateResponse({ status: 200, body })).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed",
  });
});

test("creates the workspace through a temporary host", async () => {
  const effects = fakeEffects({
    logText: "dsh web: http://127.0.0.1:4321/?token=abc",
    running: true,
    authResponse: { status: 200, body: "", setCookie: "sid=abc; Path=/" },
    createResponse: { status: 200, body: SUCCESS_BODY },
  });

  const result = await createWorkspace({ effects, directory: "/work/repo", title: "repo" });

  expect(result).toStrictEqual({
    kind: "created",
    workspace: { workspaceId: "ws1", path: "/work/repo" },
  });
  expect(effects.stop).toHaveBeenCalledTimes(1);
  expect(effects.remove).toHaveBeenCalledTimes(1);
  expect(effects.calls.at(1)).toStrictEqual({
    url: "http://127.0.0.1:4321/api/workspace/create",
    method: "POST",
    body: createRequestBody("/work/repo", "repo"),
    cookie: "sid=abc",
  });
});

test("sends no cookie when the host sets none", async () => {
  const effects = fakeEffects({
    logText: "dsh web: http://127.0.0.1:4321/?token=abc",
    running: true,
    authResponse: { status: 200, body: "" },
    createResponse: { status: 200, body: SUCCESS_BODY },
  });

  await createWorkspace({ effects, directory: "/work/repo", title: "repo" });

  expect(effects.calls.at(1)?.cookie).toBeUndefined();
});

test("reports a host that never prints a url", async () => {
  const effects = fakeEffects({
    logText: "",
    running: true,
    authResponse: { status: 200, body: "" },
    createResponse: { status: 200, body: SUCCESS_BODY },
  });

  const result = await createWorkspace({ effects, directory: "/work/repo", title: "repo" });

  expect(result).toStrictEqual({
    kind: "error",
    message: "dsh web did not print its URL within 60s",
  });
  expect(effects.stop).toHaveBeenCalledTimes(1);
});

test("stops waiting once the host has exited", async () => {
  const effects = fakeEffects({
    logText: "",
    running: false,
    authResponse: { status: 200, body: "" },
    createResponse: { status: 200, body: SUCCESS_BODY },
  });

  const result = await createWorkspace({ effects, directory: "/work/repo", title: "repo" });

  expect(result).toStrictEqual({
    kind: "error",
    message: "dsh web did not print its URL within 60s",
  });
});

test("reports an authentication failure", async () => {
  const effects = fakeEffects({
    logText: "dsh web: http://127.0.0.1:4321/?token=abc",
    running: true,
    authResponse: { status: 401, body: "" },
    createResponse: { status: 200, body: SUCCESS_BODY },
  });

  const result = await createWorkspace({ effects, directory: "/work/repo", title: "repo" });

  expect(result).toStrictEqual({ kind: "error", message: "could not authenticate to the Host" });
  expect(effects.remove).toHaveBeenCalledTimes(1);
});

test("falls back when the host error is not an object", () => {
  expect(
    parseCreateResponse({ status: 200, body: JSON.stringify({ result: { ok: false, error: 5 } }) }),
  ).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed",
  });
});

test("falls back when the envelope result is not an object", () => {
  expect(parseCreateResponse({ status: 200, body: JSON.stringify({ result: [] }) })).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed",
  });
});

test("falls back when the value is not an object", () => {
  expect(
    parseCreateResponse({
      status: 200,
      body: JSON.stringify({ result: { ok: true, value: "x" } }),
    }),
  ).toStrictEqual({
    kind: "error",
    message: "workspace/create request failed",
  });
});
