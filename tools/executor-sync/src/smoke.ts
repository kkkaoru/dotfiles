// Runs with Bun. Isolated integration smoke: synthetic DBs and age keys in memory.
// Never opens the real ~/.executor, contacts R2, or accesses real Keychain items.
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { type Client, createClient } from "@libsql/client";
import { Adapter } from "./adapter";
import { requiredCommand } from "./io";
import { type Config, check, SafeError, type Snapshot } from "./model";
import { Storage } from "./storage";

const DEFINITIONS: Record<string, string> = {
  integration:
    "slug TEXT, plugin_id TEXT, config TEXT, created_at INTEGER, updated_at INTEGER, config_revised_at BLOB",
  connection:
    "integration TEXT, name TEXT, template TEXT, provider TEXT, item_ids TEXT, refresh_item_id TEXT, expires_at BLOB, owner TEXT, subject TEXT, credential_write TEXT, last_health TEXT, tools_synced_at BLOB",
  oauth_client:
    "slug TEXT, client_secret_item_id TEXT, credential_write TEXT, owner TEXT, subject TEXT, created_at INTEGER",
  tool_policy:
    "id TEXT, pattern TEXT, action TEXT, position TEXT, owner TEXT, subject TEXT, created_at INTEGER, updated_at INTEGER",
  tool: "name TEXT",
  definition: "name TEXT",
};
async function database(path: string): Promise<Client> {
  const client: Client = createClient({ url: pathToFileURL(path).href });
  await client.execute("PRAGMA journal_mode = WAL");
  await client.batch(
    Object.entries(DEFINITIONS).map(
      ([table, columns]) =>
        `CREATE TABLE ${table} (${columns}, tenant TEXT NOT NULL, row_id TEXT PRIMARY KEY)`,
    ),
    "write",
  );
  return client;
}
async function run(): Promise<void> {
  const directory: string = await mkdtemp(
    join(tmpdir(), "executor-sync-smoke-"),
  );
  try {
    const a: string = `${directory}/a`;
    const b: string = `${directory}/b`;
    await Promise.all([mkdir(a), mkdir(b)]);
    const source: Client = await database(`${a}/data.db`);
    const destination: Client = await database(`${b}/data.db`);
    try {
      // Use named columns so schema expansion cannot accidentally reorder values.
      await source.execute({
        sql: "INSERT OR REPLACE INTO integration (slug,plugin_id,config,created_at,updated_at,config_revised_at,tenant,row_id) VALUES (?,?,?,?,?,?,?,?)",
        args: [
          "demo",
          "mcp",
          '{"command":"/opt/homebrew/bin/bun","args":["/Users/alice/work/dotfiles/demo.ts"]}',
          1,
          1,
          Buffer.from("1"),
          "dotfiles-fake",
          "row-i",
        ],
      });
      await source.execute({
        sql: "INSERT INTO connection (integration,name,template,provider,item_ids,refresh_item_id,expires_at,owner,subject,tenant,row_id) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        args: [
          "demo",
          "default",
          "oauth2",
          "file",
          '{"token":"access"}',
          "refresh",
          Buffer.from("1893456000000"),
          "user",
          "local",
          "dotfiles-fake",
          "row-c",
        ],
      });
      await source.execute({
        sql: "INSERT INTO oauth_client (slug,client_secret_item_id,owner,subject,created_at,tenant,row_id) VALUES (?,?,?,?,?,?,?)",
        args: ["demo", "client", "user", "local", 1, "dotfiles-fake", "row-o"],
      });
      await source.execute({
        sql: "INSERT INTO tool_policy (id,pattern,action,position,owner,subject,created_at,updated_at,tenant,row_id) VALUES (?,?,?,?,?,?,?,?,?,?)",
        args: [
          "read",
          "demo.**",
          "allow",
          "1",
          "user",
          "local",
          1,
          1,
          "dotfiles-fake",
          "row-p",
        ],
      });
      const sourceAdapter: Adapter = new Adapter({
        dataDir: a,
        paths: {
          home: "/Users/alice",
          repo: "/Users/alice/work/dotfiles",
          commands: { bun: "/opt/homebrew/bin/bun" },
        },
      });
      const targetAdapter: Adapter = new Adapter({
        dataDir: b,
        paths: {
          home: "/Users/bob",
          repo: "/Users/bob/other/dotfiles",
          commands: { bun: "/usr/local/bin/bun" },
        },
      });
      const { tenantFor } = await import("./model");
      await source.batch(
        Object.keys(DEFINITIONS).map((table) => ({
          sql: `UPDATE ${table} SET tenant = ?`,
          args: [tenantFor(sourceAdapter.options.paths.repo)],
        })),
        "write",
      );
      await writeFile(
        `${a}/auth.json`,
        JSON.stringify({
          access: "TEST-ACCESS",
          refresh: "TEST-REFRESH",
          client: "TEST-CLIENT-SECRET",
          unrelated: "DO-NOT-EXPORT",
        }),
        { mode: 0o600 },
      );
      await writeFile(
        `${b}/auth.json`,
        JSON.stringify({ unrelated: "KEEP-LOCAL" }),
        { mode: 0o600 },
      );
      const snapshot: Snapshot = await sourceAdapter.export();
      check(
        Object.keys(snapshot.secrets).length === 3,
        "Smoke secret allowlist failed",
      );
      const keygen: string | null = Bun.which("age-keygen");
      const age: string | null = Bun.which("age");
      check(keygen && age, "age tools required for offline smoke");
      const generated: string = (
        await requiredCommand({ executable: keygen, args: [], input: "" })
      ).toString();
      const identity: string | undefined = generated
        .split("\n")
        .find((line) => line.startsWith("AGE-SECRET-KEY-1"));
      check(identity, "Smoke key generation failed");
      const recipient: string = (
        await requiredCommand({
          executable: keygen,
          args: ["-y"],
          input: `${identity}\n`,
        })
      )
        .toString()
        .trim();
      const config: Config = {
        format: 1,
        enabled: false,
        accountId: "a".repeat(32),
        bucket: "executor-config-sync",
        group: "personal",
        intervalSeconds: 30,
        ageRecipient: recipient,
      };
      const storage: Storage = new Storage({
        config,
        credentials: { accessKeyId: "TEST", secretAccessKey: "TEST", identity },
        device: "11111111-1111-4111-8111-111111111111",
        age,
      });
      try {
        const encrypted: Buffer = await storage.encrypt(
          storage.envelope(snapshot),
        );
        check(
          !encrypted.toString().includes("TEST-REFRESH"),
          "Smoke encryption failed",
        );
        const restored = await storage.decrypt(encrypted);
        await targetAdapter.importStopped(
          restored.snapshot,
          async (previous) => {
            check(
              previous.tables.connection?.length === 0,
              "Smoke backup was not pre-import",
            );
          },
        );
        const exported: Snapshot = await targetAdapter.export();
        check(
          exported.tables.integration?.length === 1 &&
            exported.tables.connection?.length === 1 &&
            exported.tables.tool_policy?.[0]?.action === "allow",
          "Smoke settings import failed",
        );
        check(
          Object.values(exported.secrets).sort().join(",") ===
            "TEST-ACCESS,TEST-CLIENT-SECRET,TEST-REFRESH",
          "Smoke credential re-keying failed",
        );
        const expiry = await destination.execute(
          "SELECT CAST(expires_at AS TEXT) AS value FROM connection",
        );
        check(
          expiry.rows[0]?.value === "1893456000000",
          "Smoke OAuth expiry roundtrip failed",
        );
        const owner: Client = createClient({
          url: pathToFileURL(`${b}/data.db.owner-lock`).href,
        });
        try {
          await owner.execute("BEGIN EXCLUSIVE");
          const refused: boolean = await targetAdapter
            .importStopped(snapshot, async () => {
              throw new Error("Backup must not run with a live owner");
            })
            .then(
              () => false,
              () => true,
            );
          check(refused, "Smoke live-daemon safety gate failed");
        } finally {
          owner.close();
        }
      } finally {
        storage.client.destroy();
      }
    } finally {
      source.close();
      destination.close();
    }
    console.log(
      "PASS: two isolated DBs, portable paths, credentials, OAuth expiry, policies, age roundtrip and live-owner write refusal. No real credentials or services used.",
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
run()
  .then(() => process.exit(0))
  .catch((error: unknown) => {
    console.error(
      error instanceof SafeError
        ? error.message
        : "Offline smoke failed; raw secret-bearing diagnostics suppressed.",
    );
    if (
      error !== null &&
      typeof error === "object" &&
      "code" in error &&
      typeof error.code === "string" &&
      /^[A-Z_0-9]+$/.test(error.code)
    )
      console.error(error.code);
    process.exit(1);
  });
