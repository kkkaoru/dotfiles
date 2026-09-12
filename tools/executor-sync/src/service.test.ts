// Runs with Bun. Existing-service lifecycle, filesystem, age and HTTP are mocked.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Adapter } from "./adapter";
import { PATHS, snapshot } from "./fixtures";
import { checkAge, Service } from "./service";
import { Storage } from "./storage";

const mocks = vi.hoisted(() => ({
  required: vi.fn(),
  command: vi.fn(),
  read: vi.fn(),
  write: vi.fn(),
  rm: vi.fn(),
  fetch: vi.fn(),
}));
vi.mock("./io", () => ({
  requiredCommand: mocks.required,
  command: mocks.command,
  readOptional: mocks.read,
  atomicWrite: mocks.write,
}));
vi.mock("node:fs/promises", () => ({ rm: mocks.rm }));
vi.mock("node:os", () => ({ userInfo: () => ({ uid: 501 }) }));
function service(): Service {
  const adapter: Adapter = new Adapter({
    dataDir: "/mock/executor",
    paths: PATHS,
  });
  vi.spyOn(adapter, "importStopped").mockImplementation(
    async (_value, backup) => {
      await backup(snapshot());
    },
  );
  const storage: Storage = new Storage({
    config: {
      format: 1,
      enabled: false,
      accountId: "a".repeat(32),
      bucket: "executor-config-sync",
      group: "personal",
      intervalSeconds: 30,
      ageRecipient: `age1${"q".repeat(58)}`,
    },
    credentials: {
      accessKeyId: "TEST",
      secretAccessKey: "TEST",
      identity: "TEST",
    },
    device: "11111111-1111-4111-8111-111111111111",
    age: "/mock/age",
  });
  vi.spyOn(storage, "encrypt").mockResolvedValue(Buffer.from("CIPHER"));
  return new Service({
    home: PATHS.home,
    repo: PATHS.repo,
    dataDir: "/mock/executor",
    stateDir: "/mock/state",
    adapter,
    storage,
  });
}
function manifest(origin: string, scope: string): string {
  return JSON.stringify({
    scopeDir: scope,
    connection: {
      origin,
      auth: { kind: "bearer", token: "SYNTHETIC-LOCAL-TOKEN" },
    },
  });
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.required.mockResolvedValue(Buffer.from("executor v1.6.8\n"));
  mocks.command.mockResolvedValue({ code: 1, output: Buffer.alloc(0) });
  mocks.read.mockImplementation(async (path: string) =>
    path.endsWith("/server-control/server.json")
      ? manifest("http://localhost:4789", PATHS.repo)
      : null,
  );
  mocks.write.mockResolvedValue(undefined);
  mocks.rm.mockResolvedValue(undefined);
  vi.stubGlobal("fetch", mocks.fetch);
});
afterEach(() => {
  vi.unstubAllGlobals();
});
it("accepts exactly the supported runtime", async () => {
  await service().version();
  expect(mocks.required).toHaveBeenCalledTimes(1);
});
it("refuses other runtimes", async () => {
  mocks.required.mockResolvedValue(Buffer.from("executor v2"));
  await expect(service().version()).rejects.toThrow(
    "Unsupported Executor version",
  );
});
it("does not start any service without a recovery journal", async () => {
  await service().recover();
  expect(mocks.command).not.toHaveBeenCalled();
  expect(mocks.required).not.toHaveBeenCalled();
});
it("recovers only the existing named LaunchAgent", async () => {
  mocks.read.mockResolvedValue("1");
  await service().recover();
  expect(mocks.required.mock.calls[0]?.[0].args).toStrictEqual([
    "bootstrap",
    "gui/501",
    "/Users/alice/Library/LaunchAgents/sh.executor.daemon.plist",
  ]);
  expect(mocks.rm).toHaveBeenCalledTimes(1);
});
it("does not duplicate an already registered daemon", async () => {
  mocks.read.mockResolvedValue("1");
  mocks.command.mockResolvedValue({ code: 0, output: Buffer.alloc(0) });
  await service().recover();
  expect(mocks.required).not.toHaveBeenCalled();
});
it("backs up under the owner lock and queues tool indexing", async () => {
  const target: Service = service();
  await target.import(snapshot());
  expect(target.options.adapter.importStopped).toHaveBeenCalledTimes(1);
  expect(mocks.required.mock.calls[1]?.[0].args).toStrictEqual([
    "bootout",
    "gui/501",
    "/Users/alice/Library/LaunchAgents/sh.executor.daemon.plist",
  ]);
  expect(mocks.write).toHaveBeenCalledTimes(3);
});
it("attempts recovery even if import fails", async () => {
  const target: Service = service();
  mocks.read.mockImplementation(async (path: string) =>
    path.endsWith("/server-control/server.json")
      ? manifest("http://localhost:4789", PATHS.repo)
      : "1",
  );
  vi.mocked(target.options.adapter.importStopped).mockRejectedValue(
    new Error("schema"),
  );
  await expect(target.import(snapshot())).rejects.toThrow("schema");
  expect(mocks.required.mock.calls[2]?.[0].args).toStrictEqual([
    "bootstrap",
    "gui/501",
    "/Users/alice/Library/LaunchAgents/sh.executor.daemon.plist",
  ]);
});
it("rejects invalid connections before stopping anything", async () => {
  await expect(
    service().import({
      ...snapshot(),
      tables: { ...snapshot().tables, connection: [{}] },
    }),
  ).rejects.toThrow("Invalid connection rebuild queue");
  expect(mocks.write).not.toHaveBeenCalled();
});
it("does nothing without an indexing queue", async () => {
  await service().rebuild();
  expect(mocks.fetch).not.toHaveBeenCalled();
});
it("does nothing with an empty indexing queue", async () => {
  mocks.read.mockResolvedValue("[]");
  await service().rebuild();
  expect(mocks.fetch).not.toHaveBeenCalled();
});
it("rejects malformed queues", async () => {
  mocks.read.mockResolvedValue("{}");
  await expect(service().rebuild()).rejects.toThrow(
    "Invalid local rebuild queue",
  );
});
it("refuses nonlocal management origins without leaking bearer credentials", async () => {
  mocks.read
    .mockResolvedValueOnce(
      '[{"owner":"user","integration":"demo","name":"default"}]',
    )
    .mockResolvedValueOnce(manifest("https://evil.example/", PATHS.repo));
  await expect(service().rebuild()).rejects.toThrow(
    "Refusing non-local management endpoint",
  );
  expect(mocks.fetch).not.toHaveBeenCalled();
});
it("refuses another tenant's running server", async () => {
  mocks.read
    .mockResolvedValueOnce(
      '[{"owner":"user","integration":"demo","name":"default"}]',
    )
    .mockResolvedValueOnce(manifest("http://localhost:4789", "/another"));
  await expect(service().rebuild()).rejects.toThrow(
    "Executor server scope mismatch",
  );
});
it("retains queue while the server is not ready", async () => {
  mocks.read
    .mockResolvedValueOnce(
      '[{"owner":"user","integration":"demo","name":"default"}]',
    )
    .mockResolvedValueOnce(null);
  await expect(service().rebuild()).rejects.toThrow("Executor not ready");
  expect(mocks.write).not.toHaveBeenCalled();
});
it("refreshes only the local management endpoint and acknowledges nonempty tools", async () => {
  mocks.read
    .mockResolvedValueOnce(
      '[{"owner":"user","integration":"demo","name":"default"}]',
    )
    .mockResolvedValueOnce(manifest("http://localhost:4789", PATHS.repo));
  mocks.fetch.mockResolvedValue(Response.json([{ name: "test" }]));
  await service().rebuild();
  expect(mocks.fetch.mock.calls[0]?.[0]).toBe(
    "http://localhost:4789/api/connections/user/demo/default/refresh",
  );
  expect(mocks.write.mock.calls[0]?.[1]).toBe("[]");
});
it("does not discard failed indexing", async () => {
  mocks.read
    .mockResolvedValueOnce(
      '[{"owner":"user","integration":"demo","name":"default"}]',
    )
    .mockResolvedValueOnce(manifest("http://localhost:4789", PATHS.repo));
  mocks.fetch.mockResolvedValue(new Response("failed", { status: 500 }));
  await service().rebuild();
  expect(mocks.write.mock.calls[0]?.[1]).toBe(
    '[{"owner":"user","integration":"demo","name":"default"}]',
  );
});
it("does not discard empty tool results", async () => {
  mocks.read
    .mockResolvedValueOnce(
      '[{"owner":"user","integration":"demo","name":"default"}]',
    )
    .mockResolvedValueOnce(manifest("http://localhost:4789", PATHS.repo));
  mocks.fetch.mockResolvedValue(Response.json([]));
  await service().rebuild();
  expect(mocks.write.mock.calls[0]?.[1]).toBe(
    '[{"owner":"user","integration":"demo","name":"default"}]',
  );
});
it("does not discard network failures", async () => {
  mocks.read
    .mockResolvedValueOnce(
      '[{"owner":"user","integration":"demo","name":"default"}]',
    )
    .mockResolvedValueOnce(manifest("http://localhost:4789", PATHS.repo));
  mocks.fetch.mockRejectedValue(new Error("timeout"));
  await service().rebuild();
  expect(mocks.write.mock.calls[0]?.[1]).toBe(
    '[{"owner":"user","integration":"demo","name":"default"}]',
  );
});
it("checks age availability", async () => {
  mocks.command.mockResolvedValue({ code: 0, output: Buffer.from("version") });
  await checkAge("/mock/age");
});
it("refuses a broken age installation", async () => {
  await expect(checkAge("/mock/age")).rejects.toThrow("age is not installed");
});
