// Runs with Bun. CLI boundaries use mock files, native clients, Keychain and services.
import { createHash } from "node:crypto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { main } from "./cli";
import type { SyncPorts } from "./engine";
import { envelope, snapshot } from "./fixtures";
import { fingerprint, type Paths } from "./model";

interface AdapterConfiguration {
  paths: Paths;
}

const mocks = vi.hoisted(() => ({
  files: new Map<string, string>(),
  read: vi.fn(),
  write: vi.fn(),
  realpath: vi.fn(),
  mkdir: vi.fn(),
  required: vi.fn(),
  exportLocal: vi.fn(),
  pathCommands: vi.fn(),
  importLocal: vi.fn(),
  recover: vi.fn(),
  version: vi.fn(),
  rebuild: vi.fn(),
  remote: vi.fn(),
  publish: vi.fn(),
  history: vi.fn(),
  getCredentials: vi.fn(),
  setCredentials: vi.fn(),
  execute: vi.fn(),
  close: vi.fn(),
  destroy: vi.fn(),
  syncOnce: vi.fn(),
  which: vi.fn(),
  stdin: vi.fn(),
  profile: vi.fn(),
  initialize: vi.fn(),
  pairInit: vi.fn(),
  pairExport: vi.fn(),
  pairImport: vi.fn(),
  file: vi.fn(),
}));
vi.mock("node:fs/promises", () => ({
  realpath: mocks.realpath,
  mkdir: mocks.mkdir,
}));
vi.mock("node:os", () => ({ homedir: () => "/home/alice" }));
vi.mock("@libsql/client", () => ({
  createClient: () => ({ execute: mocks.execute, close: mocks.close }),
}));
vi.mock("./io", () => ({
  readOptional: mocks.read,
  atomicWrite: mocks.write,
  requiredCommand: mocks.required,
}));
vi.mock("./adapter", () => ({
  Adapter: class {
    constructor(options: AdapterConfiguration) {
      mocks.pathCommands(options.paths.commands);
    }
    export = mocks.exportLocal;
  },
}));
vi.mock("./service", () => ({
  checkAge: async () => undefined,
  Service: class {
    version = mocks.version;
    recover = mocks.recover;
    import = mocks.importLocal;
    rebuild = mocks.rebuild;
  },
}));
vi.mock("./engine", () => ({ syncOnce: mocks.syncOnce }));
vi.mock("./vault", () => ({
  readProfile: mocks.profile,
  saveProfile: mocks.setCredentials,
  initialize: mocks.initialize,
  pairingRecipient: mocks.pairInit,
  exportPairing: mocks.pairExport,
  importPairing: mocks.pairImport,
  PUBLIC_CONFIG: '{"format":2}\n',
}));
vi.mock("./mcp-storage", () => {
  return {
    McpStorage: class {
      verifyEndpoint = async () => undefined;
      client = { destroy: mocks.destroy };
      read = mocks.remote;
      publish = mocks.publish;
      history = mocks.history;
    },
  };
});
const originalArgs: string[] = [...process.argv];
function config(enabled: boolean): string {
  return JSON.stringify({
    format: 1,
    enabled,
    accountId: "a".repeat(32),
    bucket: "executor-config-sync",
    group: "personal",
    intervalSeconds: 30,
    ageRecipient: `age1${"q".repeat(58)}`,
  });
}
async function run(...args: string[]): Promise<void> {
  process.argv = ["bun", "cli.ts", ...args];
  await main();
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.files.clear();
  vi.stubEnv("EXECUTOR_DATA_DIR", "");
  vi.stubEnv("EXECUTOR_KEYCHAIN_SERVICE_NAME", "");
  vi.stubGlobal("Bun", {
    which: mocks.which,
    stdin: { text: mocks.stdin },
    file: mocks.file,
  });
  vi.spyOn(console, "log").mockImplementation(() => undefined);
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  mocks.which.mockImplementation((name: string) => `/bin/${name}`);
  mocks.realpath.mockImplementation(async (path: string) =>
    path === "/home/alice"
      ? path
      : path === "/home/alice/.executor"
        ? "/repo/.executor"
        : "/repo",
  );
  mocks.read.mockImplementation(
    async (path: string) => mocks.files.get(path) ?? null,
  );
  mocks.write.mockImplementation(async (path: string, text: string) => {
    mocks.files.set(path, text);
  });
  mocks.files.set("/repo/.executor/sync.json", '{"format":2}\n');
  mocks.profile.mockImplementation(() => ({
    version: 2,
    config: JSON.parse(config(true)),
    identity: "TEST",
    authKey: "0".repeat(64),
    trustedAfter: null,
  }));
  mocks.initialize.mockImplementation(async (account: string) => {
    if (!/^[a-f0-9]{32}$/.test(account)) throw new Error("Invalid account ID");
  });
  mocks.execute.mockResolvedValue(undefined);
  mocks.close.mockReturnValue(undefined);
  mocks.version.mockResolvedValue(undefined);
  mocks.recover.mockResolvedValue(undefined);
  mocks.rebuild.mockResolvedValue(undefined);
  mocks.importLocal.mockResolvedValue(undefined);
  mocks.exportLocal.mockResolvedValue(snapshot());
  mocks.remote.mockResolvedValue(null);
  mocks.publish.mockResolvedValue("tag");
  mocks.history.mockResolvedValue(["history/test.age"]);
  mocks.syncOnce.mockResolvedValue({
    action: "unchanged",
    state: { localHash: null, remoteTag: null, lastSuccess: null },
  });
  mocks.required.mockResolvedValue(Buffer.from("executor v1.6.8\n"));
  mocks.getCredentials.mockReturnValue({
    accessKeyId: "TEST",
    secretAccessKey: "TEST",
    identity: "TEST",
  });
});
afterEach(() => {
  process.argv = originalArgs;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});
it("help has no filesystem or credential effects", async () => {
  await run("--help");
  expect(mocks.realpath).not.toHaveBeenCalled();
  expect(mocks.getCredentials).not.toHaveBeenCalled();
});
it("status never accesses credentials", async () => {
  await run("status");
  expect(mocks.getCredentials).not.toHaveBeenCalled();
  expect(mocks.exportLocal).not.toHaveBeenCalled();
});
it("doctor is read-only and outputs only counts", async () => {
  await run("doctor");
  expect(mocks.exportLocal).toHaveBeenCalledTimes(1);
  expect(mocks.write).not.toHaveBeenCalled();
  expect(mocks.importLocal).not.toHaveBeenCalled();
});
it("maps the installed optional Peekaboo executable without requiring UI use", async () => {
  await run("doctor");
  expect(mocks.pathCommands).toHaveBeenCalledWith({
    bun: "/bin/bun",
    bunx: "/bin/bunx",
    ctx: "/bin/ctx",
    peekaboo: "/bin/peekaboo",
  });
});
it("does not require Peekaboo when it is not installed", async () => {
  mocks.which.mockImplementation((name: string) =>
    name === "peekaboo" ? null : `/bin/${name}`,
  );
  await run("doctor");
  expect(mocks.pathCommands).toHaveBeenCalledWith({
    bun: "/bin/bun",
    bunx: "/bin/bunx",
    ctx: "/bin/ctx",
  });
});
it("doctor checks the actual binary version", async () => {
  mocks.required.mockResolvedValue(Buffer.from("executor v2"));
  await expect(run("doctor")).rejects.toThrow("Unsupported Executor version");
});
it("refuses custom data roots", async () => {
  vi.stubEnv("EXECUTOR_DATA_DIR", "/other");
  await expect(run("doctor")).rejects.toThrow("Custom Executor");
});
it("refuses missing executables", async () => {
  mocks.which.mockReturnValue(null);
  await expect(run("doctor")).rejects.toThrow("Required local executable");
});
it("refuses an unrelated home directory", async () => {
  mocks.realpath.mockResolvedValue("/other");
  await expect(run("doctor")).rejects.toThrow("Executor home must link");
});
it("does not sync when disabled", async () => {
  mocks.profile.mockReturnValue({ config: JSON.parse(config(false)) });
  await expect(run("sync")).rejects.toThrow("Sync is disabled");
  expect(mocks.getCredentials).not.toHaveBeenCalled();
});
it("disabled hooks leave local execution untouched", async () => {
  mocks.profile.mockReturnValue({ config: JSON.parse(config(false)) });
  await run("hook", "before");
  expect(mocks.execute).not.toHaveBeenCalled();
});
it("runs a transient sync with existing-service recovery", async () => {
  await run("sync");
  expect(mocks.syncOnce).toHaveBeenCalledTimes(1);
  expect(mocks.recover).toHaveBeenCalledTimes(1);
  expect(mocks.rebuild).toHaveBeenCalledTimes(1);
  expect(mocks.destroy).toHaveBeenCalledTimes(1);
  expect(mocks.close).toHaveBeenCalledTimes(1);
});
it("wires every sync port without exposing credentials", async () => {
  mocks.syncOnce.mockImplementationOnce(async (ports: SyncPorts) => {
    await ports.exportLocal();
    await ports.importLocal(snapshot());
    await ports.readRemote();
    await ports.publish(snapshot());
    await ports.saveState({
      localHash: "hash",
      remoteTag: "tag",
      lastSuccess: ports.now(),
    });
    return { action: "uploaded", state: {} };
  });
  await run("sync");
  expect(mocks.exportLocal).toHaveBeenCalledTimes(1);
  expect(mocks.importLocal).toHaveBeenCalledTimes(1);
  expect(mocks.remote).toHaveBeenCalledTimes(1);
  expect(mocks.publish).toHaveBeenCalledTimes(1);
});
it("uses a saved device and state", async () => {
  mocks.files.set(
    "/repo/.executor/sync-local/device-id",
    "11111111-1111-4111-8111-111111111111",
  );
  mocks.files.set(
    "/repo/.executor/sync-local/state.json",
    '{"localHash":"old","remoteTag":"tag","lastSuccess":null}',
  );
  await run("sync");
  expect(mocks.syncOnce.mock.calls[0]?.[1]).toStrictEqual({
    localHash: "old",
    remoteTag: "tag",
    lastSuccess: null,
  });
});
it("rejects malformed local state", async () => {
  mocks.files.set("/repo/.executor/sync-local/state.json", "{}");
  await expect(run("sync")).rejects.toThrow("Invalid local sync state");
});
it("rejects malformed device IDs", async () => {
  mocks.files.set("/repo/.executor/sync-local/device-id", "bad");
  await expect(run("sync")).rejects.toThrow("Invalid device ID");
});
it("refuses missing public configuration", async () => {
  mocks.files.delete("/repo/.executor/sync.json");
  await expect(run("sync")).rejects.toThrow("Sync configuration missing");
});
it("refuses invalid public configuration", async () => {
  mocks.files.set("/repo/.executor/sync.json", "{}");
  await expect(run("sync")).rejects.toThrow(
    "Public sync config must contain only format: 2",
  );
});
it("throttles remote reads before CLI calls", async () => {
  mocks.files.set(
    "/repo/.executor/sync-local/state.json",
    JSON.stringify({
      localHash: "old",
      remoteTag: "tag",
      lastSuccess: new Date().toISOString(),
    }),
  );
  await run("hook", "before");
  expect(mocks.syncOnce).not.toHaveBeenCalled();
});
it("does not upload unchanged local settings after calls", async () => {
  mocks.files.set(
    "/repo/.executor/sync-local/state.json",
    JSON.stringify({
      localHash: fingerprint(snapshot()),
      remoteTag: "tag",
      lastSuccess: null,
    }),
  );
  await run("hook", "after");
  expect(mocks.syncOnce).not.toHaveBeenCalled();
});
it("uploads changes after calls", async () => {
  mocks.files.set(
    "/repo/.executor/sync-local/state.json",
    '{"localHash":"old","remoteTag":"tag","lastSuccess":null}',
  );
  await run("hook", "after");
  expect(mocks.syncOnce).toHaveBeenCalledTimes(1);
});
it("hooks retain local execution on failure and record safe status", async () => {
  mocks.syncOnce.mockRejectedValue(new Error("DO-NOT-PRINT-SECRET"));
  await run("hook", "before");
  expect(
    mocks.files
      .get("/repo/.executor/sync-local/status.json")
      ?.includes("DO-NOT-PRINT-SECRET"),
  ).toBe(false);
  expect(console.error).toHaveBeenCalledTimes(1);
});
it("rejects invalid hook phases", async () => {
  await expect(run("hook", "other")).rejects.toThrow("Invalid sync hook phase");
});
it("enables only after accessing the bucket", async () => {
  await run("enable");
  expect(mocks.remote).toHaveBeenCalledTimes(1);
  expect(mocks.write).toHaveBeenCalled();
});
it("does not enable if remote validation fails", async () => {
  mocks.remote.mockRejectedValue(new Error("wrong key"));
  await expect(run("enable")).rejects.toThrow("wrong key");
  expect(mocks.files.get("/repo/.executor/sync.json")).not.toBeUndefined();
});
it("disables without accessing remote credentials", async () => {
  await run("disable");
  expect(mocks.getCredentials).not.toHaveBeenCalled();
});
it("lists history without changing settings", async () => {
  await run("history");
  expect(mocks.history).toHaveBeenCalledTimes(1);
  expect(mocks.importLocal).not.toHaveBeenCalled();
});
it("restores a selected snapshot and publishes a new latest", async () => {
  mocks.remote.mockResolvedValue({ tag: "old", envelope: envelope() });
  await run("restore", "history/2026-01-01T00-00-00.000Z-1111.age");
  expect(mocks.importLocal).toHaveBeenCalledTimes(1);
  expect(mocks.publish).toHaveBeenCalledTimes(1);
});
it("refuses missing restore identifiers", async () => {
  await expect(run("restore")).rejects.toThrow("History ID required");
});
it("rejects traversal in restore identifiers", async () => {
  await expect(run("restore", "../latest.age")).rejects.toThrow(
    "Invalid history ID",
  );
});
it("does not import absent history entries", async () => {
  await expect(
    run("restore", "history/2026-01-01T00-00-00.000Z-1111.age"),
  ).rejects.toThrow("History entry not found");
  expect(mocks.importLocal).not.toHaveBeenCalled();
});
it("rejects malformed secret input", async () => {
  mocks.stdin.mockResolvedValue("bad");
  await expect(run("configure")).rejects.toThrow("Invalid account ID");
  expect(mocks.setCredentials).not.toHaveBeenCalled();
});
it("stores credentials via Keychain and public data separately", async () => {
  mocks.stdin.mockResolvedValue("a".repeat(32));
  mocks.required.mockResolvedValue(Buffer.from(`age1${"q".repeat(58)}`));
  await run("configure");
  expect(mocks.initialize).toHaveBeenCalledTimes(1);
  expect(mocks.files.get("/repo/.executor/sync.json")).toBe('{"format":2}\n');
});
it("pair-init outputs only a public recipient", async () => {
  mocks.pairInit.mockResolvedValue("PUBLIC-RECIPIENT");
  await run("pair-init");
  expect(console.log).toHaveBeenCalledWith("PUBLIC-RECIPIENT");
  expect(mocks.exportLocal).not.toHaveBeenCalled();
});
it("exports ciphertext only and refuses overwriting an existing file", async () => {
  mocks.pairExport.mockResolvedValue(Buffer.from("ENCRYPTED"));
  await run("pair-export", "public-key", "/tmp/pair.age");
  expect(mocks.pairExport).toHaveBeenCalledWith("public-key", "/bin/age");
  await expect(
    run("pair-export", "public-key", "/tmp/pair.age"),
  ).rejects.toThrow("overwrite");
  await expect(run("pair-export")).rejects.toThrow("required");
});
it("pair-import requires a separately authenticated sender digest", async () => {
  await expect(run("pair-import")).rejects.toThrow("file required");
  await expect(run("pair-import", "/tmp/pair.age")).rejects.toThrow(
    "Trusted sender",
  );
  mocks.file.mockReturnValue({
    size: 9,
    arrayBuffer: async () => Buffer.from("ENCRYPTED"),
  });
  await expect(
    run("pair-import", "/tmp/pair.age", "0".repeat(64)),
  ).rejects.toThrow("digest mismatch");
  expect(mocks.pairImport).not.toHaveBeenCalled();
  await run(
    "pair-import",
    "/tmp/pair.age",
    createHash("sha256").update("ENCRYPTED").digest("hex"),
  );
  expect(mocks.pairImport).toHaveBeenCalledTimes(1);
  expect(mocks.files.get("/repo/.executor/sync.json")).toBe('{"format":2}\n');
});
it("does not permit public configuration to select a destination or enable syncing", async () => {
  mocks.files.set(
    "/repo/.executor/sync.json",
    '{"format":2,"enabled":true,"accountId":"attacker"}',
  );
  await expect(run("sync")).rejects.toThrow("Public sync config");
  expect(mocks.syncOnce).not.toHaveBeenCalled();
});
it("rejects unsupported commands", async () => {
  await expect(run("watch")).rejects.toThrow("Unknown command");
});
