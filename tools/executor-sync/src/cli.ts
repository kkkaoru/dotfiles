// Runs with Bun. Never print raw exceptions, provider responses, tokens or snapshots.
import { createHash, randomUUID } from "node:crypto";
import { mkdir, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createClient } from "@libsql/client";
import { z } from "zod";
import { Adapter } from "./adapter";
import { syncOnce } from "./engine";
import { atomicWrite, readOptional, requiredCommand } from "./io";
import { McpStorage as Storage } from "./mcp-storage";
import {
  type Config,
  check,
  configSchema,
  fingerprint,
  INITIAL_STATE,
  type Paths,
  SafeError,
  type Snapshot,
  type SyncState,
  stateSchema,
} from "./model";
import { checkAge, Service } from "./service";
import {
  exportPairing,
  importPairing,
  initialize,
  PUBLIC_CONFIG,
  pairingRecipient,
  readProfile,
  saveProfile,
} from "./vault";

interface Context {
  repo: string;
  home: string;
  dataDir: string;
  stateDir: string;
  configPath: string;
  paths: Paths;
  age: string;
}
async function context(): Promise<Context> {
  check(
    !process.env.EXECUTOR_DATA_DIR &&
      !process.env.EXECUTOR_KEYCHAIN_SERVICE_NAME,
    "Custom Executor data/provider overrides are not supported by sync",
  );
  const repo: string = await realpath(
    fileURLToPath(new URL("../../..", import.meta.url)),
  );
  const home: string = await realpath(homedir());
  const commands: Record<string, string> = Object.fromEntries(
    ["bun", "bunx", "ctx"].map((name) => {
      const path: string | null = Bun.which(name);
      check(path, "Required local executable not found");
      return [name, path];
    }),
  );
  // Optional UI integration: resolve the executable independently on each Mac.
  const peekaboo: string | null = Bun.which("peekaboo");
  if (peekaboo) commands.peekaboo = peekaboo;
  const age: string | null = Bun.which("age");
  check(age, "age is required");
  const dataDir: string = await realpath(`${home}/.executor`);
  check(
    dataDir === `${repo}/.executor`,
    "Executor home must link to this checkout",
  );
  return {
    repo,
    home,
    dataDir,
    stateDir: `${dataDir}/sync-local`,
    configPath: `${repo}/.executor/sync.json`,
    paths: { home, repo, commands },
    age,
  };
}
async function loadConfig(ctx: Context): Promise<Config> {
  const text: string | null = await readOptional(ctx.configPath);
  check(text, "Sync configuration missing");
  check(
    JSON.stringify(JSON.parse(text)) === JSON.stringify({ format: 2 }),
    "Public sync config must contain only format: 2; private settings belong in Keychain",
  );
  return readProfile().config;
}
async function loadState(ctx: Context): Promise<SyncState> {
  const text: string | null = await readOptional(`${ctx.stateDir}/state.json`);
  if (text === null) return INITIAL_STATE;
  const parsed = stateSchema.safeParse(JSON.parse(text));
  check(parsed.success, "Invalid local sync state");
  return parsed.data;
}
async function deviceId(ctx: Context): Promise<string> {
  const path: string = `${ctx.stateDir}/device-id`;
  const id: string | null = await readOptional(path);
  if (id !== null) {
    check(z.string().uuid().safeParse(id).success, "Invalid device ID");
    return id;
  }
  const created: string = randomUUID();
  await atomicWrite(path, created);
  return created;
}
async function withLocalLock(
  ctx: Context,
  action: () => Promise<void>,
): Promise<void> {
  await mkdir(ctx.stateDir, { recursive: true, mode: 0o700 });
  // Kernel-backed same-Mac mutex: automatically released even after a crash.
  // This is NOT a multi-Mac usage lock or a device switching protocol.
  const lock = createClient({
    url: pathToFileURL(`${ctx.stateDir}/process.lock.db`).href,
  });
  try {
    await lock.execute("PRAGMA busy_timeout = 0");
    await lock.execute("BEGIN EXCLUSIVE");
    try {
      await action();
    } finally {
      await lock.execute("ROLLBACK");
    }
  } finally {
    lock.close();
  }
}
async function configure(ctx: Context): Promise<void> {
  check(
    !process.stdin.isTTY,
    "Provide the account ID on stdin; no S3 credentials required",
  );
  const keygen: string | null = Bun.which("age-keygen");
  check(keygen, "age-keygen is required");
  await initialize((await Bun.stdin.text()).trim(), keygen);
  await atomicWrite(ctx.configPath, PUBLIC_CONFIG);
  console.log(
    "Private profile and shared keys saved in Keychain. Sync remains disabled.",
  );
}
async function storageFor(ctx: Context): Promise<Storage> {
  await loadConfig(ctx);
  const storage: Storage = new Storage({
    profile: readProfile(),
    device: await deviceId(ctx),
    age: ctx.age,
    executor: `${ctx.repo}/scripts/executor`,
  });
  await storage.verifyEndpoint();
  return storage;
}
async function runSync(ctx: Context, restore: string | null): Promise<void> {
  const config: Config = await loadConfig(ctx);
  check(
    config.enabled,
    "Sync is disabled; use executor-sync enable after reviewing the private profile",
  );
  const storage: Storage = await storageFor(ctx);
  const adapter: Adapter = new Adapter({
    dataDir: ctx.dataDir,
    paths: ctx.paths,
  });
  const service: Service = new Service({ ...ctx, adapter, storage });
  try {
    await service.version();
    await service.recover();
    if (restore !== null) {
      check(
        /^history\/[0-9TZ.:-]+-[a-f0-9-]+\.age$/.test(restore),
        "Invalid history ID",
      );
      const remote = await storage.read(restore);
      check(remote, "History entry not found");
      await service.import(remote.envelope.snapshot);
      await storage.publish(await adapter.export());
      await atomicWrite(
        `${ctx.stateDir}/state.json`,
        JSON.stringify(INITIAL_STATE),
      );
    } else {
      const result = await syncOnce(
        {
          exportLocal: () => adapter.export(),
          importLocal: (snapshot: Snapshot) => service.import(snapshot),
          readRemote: () => storage.read("latest.age"),
          publish: (snapshot: Snapshot) => storage.publish(snapshot),
          saveState: (state: SyncState) =>
            atomicWrite(`${ctx.stateDir}/state.json`, JSON.stringify(state)),
          now: () => new Date().toISOString(),
        },
        await loadState(ctx),
      );
      console.log(`Sync ${result.action}.`);
    }
    await service.rebuild();
  } finally {
    storage.client.destroy();
  }
}
async function hook(ctx: Context, phase: string | undefined): Promise<void> {
  check(phase === "before" || phase === "after", "Invalid sync hook phase");
  const config: Config = await loadConfig(ctx);
  if (!config.enabled) return;
  try {
    await withLocalLock(ctx, async () => {
      const state: SyncState = await loadState(ctx);
      if (
        phase === "before" &&
        state.lastSuccess &&
        Date.now() - Date.parse(state.lastSuccess) < 30_000
      )
        return;
      if (phase === "after" && state.localHash) {
        const local: Snapshot = await new Adapter({
          dataDir: ctx.dataDir,
          paths: ctx.paths,
        }).export();
        if (fingerprint(local) === state.localHash) return;
      }
      await runSync(ctx, null);
      await atomicWrite(
        `${ctx.stateDir}/status.json`,
        JSON.stringify({ status: "ok", checkedAt: new Date().toISOString() }),
      );
    });
  } catch {
    // Hooks must not break local tool execution or reveal secret-bearing errors.
    await atomicWrite(
      `${ctx.stateDir}/status.json`,
      JSON.stringify({
        status: "error",
        checkedAt: new Date().toISOString(),
        message: "Sync failed; run doctor and check pending tool indexing.",
      }),
    );
    console.error(
      "Settings sync failed; local Executor remains available. Run executor-sync status.",
    );
  }
}
export async function main(): Promise<void> {
  const action: string | undefined = process.argv[2];
  if (!action || action === "--help") {
    console.log(
      "executor-sync configure | pair-init | pair-export <recipient> <file> | pair-import <file> <trusted-sha256> | enable | disable | doctor | sync | status | history | restore <history/id.age> | hook before|after",
    );
    return;
  }
  const ctx: Context = await context();
  if (action === "configure") {
    await configure(ctx);
    return;
  }
  if (action === "pair-init") {
    const keygen: string | null = Bun.which("age-keygen");
    check(keygen, "age-keygen is required");
    console.log(await pairingRecipient(keygen));
    return;
  }
  if (action === "pair-export") {
    const target: string | undefined = process.argv[3];
    const output: string | undefined = process.argv[4];
    check(target && output, "Pairing recipient and output file required");
    check(
      (await readOptional(output)) === null,
      "Refusing to overwrite pairing output",
    );
    const encrypted: Buffer = await exportPairing(target, ctx.age);
    await atomicWrite(output, encrypted);
    console.log(
      `Pairing SHA-256: ${createHash("sha256").update(encrypted).digest("hex")}`,
    );
    console.log(
      "Verify recipient and this digest through an authenticated independent channel.",
    );
    return;
  }
  if (action === "pair-import") {
    const path: string | undefined = process.argv[3];
    check(path, "Encrypted pairing file required");
    const digest: string | undefined = process.argv[4];
    check(
      digest && /^[a-f0-9]{64}$/.test(digest),
      "Trusted sender SHA-256 required; obtain it independently of the bundle",
    );
    check(Bun.file(path).size <= 16384, "Pairing bundle too large");
    const encrypted: Uint8Array = new Uint8Array(
      await Bun.file(path).arrayBuffer(),
    );
    check(
      createHash("sha256").update(encrypted).digest("hex") === digest,
      "Pairing sender digest mismatch",
    );
    await importPairing(encrypted, ctx.age);
    await atomicWrite(ctx.configPath, PUBLIC_CONFIG);
    console.log(
      "Private pairing imported into Keychain. Sync remains disabled.",
    );
    return;
  }
  if (action === "enable" || action === "disable") {
    const parsed = configSchema.safeParse({
      ...(await loadConfig(ctx)),
      enabled: action === "enable",
    });
    check(parsed.success, "Configure sync before enabling");
    if (parsed.data.enabled) {
      const storage: Storage = await storageFor(ctx);
      try {
        await storage.read("latest.age");
      } finally {
        storage.client.destroy();
      }
    }
    saveProfile({ ...readProfile(), config: parsed.data });
    console.log(
      `Settings sync ${action === "enable" ? "enabled" : "disabled"}; no daemon installed.`,
    );
    return;
  }
  if (action === "status") {
    console.log(
      (await readOptional(`${ctx.stateDir}/status.json`)) ?? "Not started.",
    );
    console.log(JSON.stringify(await loadState(ctx)));
    return;
  }
  if (action === "doctor") {
    await checkAge(ctx.age);
    const output: Buffer = await requiredCommand({
      executable: `${ctx.repo}/scripts/executor`,
      args: ["--version"],
      input: "",
    });
    check(
      output.toString().trim() === "executor v1.6.8",
      "Unsupported Executor version",
    );
    const snapshot: Snapshot = await new Adapter({
      dataDir: ctx.dataDir,
      paths: ctx.paths,
    }).export();
    console.log(
      JSON.stringify({
        version: snapshot.executorVersion,
        tables: Object.fromEntries(
          Object.entries(snapshot.tables).map(([table, rows]) => [
            table,
            rows.length,
          ]),
        ),
        credentials: Object.keys(snapshot.secrets).length,
      }),
    );
    return;
  }
  if (action === "history") {
    const storage: Storage = await storageFor(ctx);
    try {
      console.log((await storage.history()).join("\n"));
    } finally {
      storage.client.destroy();
    }
    return;
  }
  if (action === "sync") {
    await withLocalLock(ctx, () => runSync(ctx, null));
    return;
  }
  if (action === "hook") {
    await hook(ctx, process.argv[3]);
    return;
  }
  if (action === "restore") {
    const id: string | undefined = process.argv[3];
    check(id, "History ID required");
    await withLocalLock(ctx, () => runSync(ctx, id));
    return;
  }
  throw new Error("Unknown command");
}
function finish(code: number): void {
  process.stdout.write("", () =>
    process.stderr.write("", () => process.exit(code)),
  );
}
if (import.meta.main) {
  // Native libSQL bindings may retain background handles under Bun. All resources
  // are closed above; this CLI is deliberately short-lived, never a daemon.
  main()
    .then(() => finish(0))
    .catch((error: unknown) => {
      console.error(
        error instanceof SafeError
          ? error.message
          : "executor-sync failed. Check configuration, Keychain access, dependencies, version, service state and bucket permissions. Raw diagnostics were suppressed.",
      );
      finish(1);
    });
}
