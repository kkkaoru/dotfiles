// Runs with Bun. Executor 1.6.8 adapter; writes are allowed ONLY while the daemon is stopped.
// Schema reference: RhysSullivan/executor f1d95f2b657316180992d5a67c24b7b76dc2b0f1.
import { randomUUID } from "node:crypto";
import { access } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import {
  type Client,
  createClient,
  type InValue,
  type Row,
  type Transaction,
} from "@libsql/client";
import { Entry } from "@napi-rs/keyring";
import { z } from "zod";
import { atomicWrite, readOptional } from "./io";
import {
  check,
  type Paths,
  type PortableRow,
  parseSnapshot,
  portableConfig,
  type Snapshot,
  stable,
  TABLES,
  tenantFor,
} from "./model";

export interface AdapterOptions {
  dataDir: string;
  paths: Paths;
}
interface SecretReference {
  provider: string;
  id: string;
}
interface ImportContext {
  snapshot: Snapshot;
  ids: Record<string, string>;
  tenant: string;
  paths: Paths;
  now: number;
}
const strings = z.record(z.string(), z.string());
const OMITTED: string[] = [
  "row_id",
  "tenant",
  "subject",
  "created_at",
  "updated_at",
  "config_revised_at",
  "last_health",
  "tools_synced_at",
  "credential_write",
];
const REBUILD_TABLES: string[] = ["tool", "definition"];
// Executor stores JSON columns as UTF-8 BLOBs in FumaDB, unlike ordinary text.
const JSON_COLUMNS: string[] = [
  "config",
  "item_ids",
  "provider_state",
  "identity_override",
  "health_check",
];

function field(row: PortableRow, key: string): string {
  const value = row[key];
  check(typeof value === "string", "Missing required settings field");
  return value;
}
function scalar(value: Row[string]): PortableRow[string] {
  if (value instanceof ArrayBuffer)
    return { encoding: "base64", data: Buffer.from(value).toString("base64") };
  if (typeof value === "bigint") {
    check(Number.isSafeInteger(Number(value)), "Unsafe database integer");
    return Number(value);
  }
  check(
    value === null ||
      typeof value === "string" ||
      (typeof value === "number" && Number.isFinite(value)),
    "Unsupported database value",
  );
  return value;
}
function stringMap(text: string): Record<string, string> {
  const parsed = strings.safeParse(JSON.parse(text));
  check(parsed.success, "Invalid credential reference map");
  return parsed.data;
}
function refs(snapshot: Snapshot): SecretReference[] {
  const connections: PortableRow[] = snapshot.tables.connection ?? [];
  const clients: PortableRow[] = snapshot.tables.oauth_client ?? [];
  return [
    ...connections.flatMap((row) => {
      const provider: string = field(row, "provider");
      check(
        provider === "file" || provider === "keychain",
        "Unsupported credential provider; no partial export",
      );
      const ids: string[] = [
        ...Object.values(stringMap(field(row, "item_ids"))),
        ...(typeof row.refresh_item_id === "string"
          ? [row.refresh_item_id]
          : []),
      ];
      return ids.map((id) => ({ provider, id }));
    }),
    ...clients.flatMap((row) =>
      typeof row.client_secret_item_id === "string"
        ? [{ provider: "file", id: row.client_secret_item_id }]
        : [],
    ),
  ];
}
async function inOrder<T>(tasks: Array<() => Promise<T>>): Promise<T[]> {
  return tasks.reduce<Promise<T[]>>(
    (previous, task) =>
      previous.then(async (results) => [...results, await task()]),
    Promise.resolve([]),
  );
}
async function schema(
  client: Client | Transaction,
): Promise<Record<string, string[]>> {
  return Object.fromEntries(
    await inOrder(
      TABLES.map((name) => async () => {
        const result = await client.execute(`PRAGMA table_info("${name}")`);
        const columns = result.rows.map((row) => {
          check(
            typeof row.name === "string" && /^[a-z_]+$/.test(row.name),
            "Unsupported settings schema",
          );
          return row.name;
        });
        check(
          columns.includes("tenant") && columns.includes("row_id"),
          "Executor settings schema missing",
        );
        return [name, columns];
      }),
    ),
  );
}
function convertedRow(table: string, row: Row, paths: Paths): PortableRow {
  const values: PortableRow = Object.fromEntries(
    Object.entries(row)
      .filter(([name]) => !OMITTED.includes(name))
      .map(([name, value]) => [
        name,
        JSON_COLUMNS.includes(name) && value instanceof ArrayBuffer
          ? Buffer.from(value).toString("utf8")
          : scalar(value),
      ]),
  );
  if (table === "integration") {
    check(
      values.plugin_id === "mcp",
      "MVP supports MCP integrations only; no partial export",
    );
    if (typeof values.config === "string")
      values.config = portableConfig(values.config, paths, "export");
  }
  return values;
}
function remappedRow(options: {
  table: string;
  row: PortableRow;
  context: ImportContext;
}): PortableRow {
  const { table, context } = options;
  const row: PortableRow = { ...options.row };
  const mapped = (provider: string, id: string): string => {
    const value: string | undefined = context.ids[`${provider}:${id}`];
    check(value, "Missing imported credential");
    return value;
  };
  if (table === "integration" && typeof row.config === "string")
    row.config = portableConfig(row.config, context.paths, "import");
  if (table === "connection") {
    const provider: string = field(row, "provider");
    row.item_ids = stable(
      Object.fromEntries(
        Object.entries(stringMap(field(row, "item_ids"))).map(([key, id]) => [
          key,
          mapped(provider, id),
        ]),
      ),
    );
    if (typeof row.refresh_item_id === "string")
      row.refresh_item_id = mapped(provider, row.refresh_item_id);
    row.provider = "file";
  }
  if (table === "oauth_client" && typeof row.client_secret_item_id === "string")
    row.client_secret_item_id = mapped("file", row.client_secret_item_id);
  return row;
}
function insertValues(options: {
  columns: string[];
  row: PortableRow;
  context: ImportContext;
}): InValue[] {
  return options.columns.map((column) => {
    if (column === "row_id") return randomUUID();
    if (column === "tenant") return options.context.tenant;
    if (column === "subject") return options.row.owner === "org" ? "" : "local";
    if (["created_at", "updated_at", "config_revised_at"].includes(column))
      return options.context.now;
    if (["last_health", "tools_synced_at", "credential_write"].includes(column))
      return null;
    const value = options.row[column];
    check(value !== undefined, "Snapshot column missing");
    if (JSON_COLUMNS.includes(column) && typeof value === "string")
      return Buffer.from(value, "utf8");
    return value !== null && typeof value === "object"
      ? Buffer.from(value.data, "base64")
      : value;
  });
}
export class Adapter {
  readonly options: AdapterOptions;
  constructor(options: AdapterOptions) {
    this.options = options;
  }
  open(): Client {
    return createClient({
      url: pathToFileURL(`${this.options.dataDir}/data.db`).href,
      intMode: "number",
    });
  }
  async export(): Promise<Snapshot> {
    await access(`${this.options.dataDir}/data.db`);
    const client: Client = this.open();
    try {
      const transaction: Transaction = await client.transaction("read");
      try {
        await transaction.execute("PRAGMA query_only = ON");
        const shape: Record<string, string[]> = await schema(transaction);
        const tables: Record<string, PortableRow[]> = Object.fromEntries(
          await inOrder(
            TABLES.map((table) => async () => {
              const result = await transaction.execute({
                sql: `SELECT * FROM "${table}" WHERE tenant = ? ORDER BY row_id`,
                args: [tenantFor(this.options.paths.repo)],
              });
              return [
                table,
                result.rows
                  .map((row) => convertedRow(table, row, this.options.paths))
                  .sort((a, b) => stable(a).localeCompare(stable(b))),
              ];
            }),
          ),
        );
        const snapshot: Snapshot = {
          format: 1,
          executorVersion: "1.6.8",
          schema: shape,
          tables,
          secrets: {},
        };
        const fileText: string | null = await readOptional(
          `${this.options.dataDir}/auth.json`,
        );
        const fileSecrets: Record<string, string> = stringMap(fileText ?? "{}");
        refs(snapshot).map(({ provider, id }) => {
          const value: string | null | undefined =
            provider === "file"
              ? fileSecrets[id]
              : new Entry("executor", id).getPassword();
          check(
            typeof value === "string",
            "Referenced credential missing; export refused",
          );
          snapshot.secrets[`${provider}:${id}`] = value;
          return null;
        });
        await transaction.commit();
        return parseSnapshot(snapshot);
      } finally {
        transaction.close();
      }
    } finally {
      client.close();
    }
  }
  // This is the SAME SQLite exclusive owner lock used by Executor 1.6.8.
  // A running daemon, or one racing to restart, prevents writes instead of risking corruption.
  async importStopped(
    value: Snapshot,
    backup: (snapshot: Snapshot) => Promise<void>,
  ): Promise<void> {
    await access(`${this.options.dataDir}/data.db`);
    const owner: Client = createClient({
      url: pathToFileURL(`${this.options.dataDir}/data.db.owner-lock`).href,
    });
    try {
      await owner.execute("PRAGMA busy_timeout = 0");
      await owner.execute("PRAGMA journal_mode = DELETE");
      await owner.execute("BEGIN EXCLUSIVE");
      try {
        await backup(await this.export());
        await this.replaceSettings(value);
      } finally {
        await owner.execute("ROLLBACK");
      }
    } finally {
      owner.close();
    }
  }
  private async replaceSettings(value: Snapshot): Promise<void> {
    const snapshot: Snapshot = parseSnapshot(value);
    const client: Client = this.open();
    const authPath: string = `${this.options.dataDir}/auth.json`;
    const previous: string | null = await readOptional(authPath);
    const original: Record<string, string> = stringMap(previous ?? "{}");
    try {
      check(
        stable(await schema(client)) === stable(snapshot.schema),
        "Schema mismatch; import refused",
      );
      check(
        (snapshot.tables.integration ?? []).every(
          (row) => row.plugin_id === "mcp",
        ),
        "Unsupported integration plugin",
      );
      TABLES.map((table) => {
        const columns: string[] = snapshot.schema[table] ?? [];
        (snapshot.tables[table] ?? []).map((row) => {
          check(
            Object.keys(row).every(
              (name) => columns.includes(name) && !OMITTED.includes(name),
            ),
            "Unexpected snapshot column",
          );
          check(
            row.owner === undefined ||
              row.owner === "org" ||
              row.owner === "user",
            "Invalid settings owner",
          );
          return null;
        });
        return null;
      });
      const references: SecretReference[] = refs(snapshot);
      check(
        references.every(
          ({ provider, id }) =>
            typeof snapshot.secrets[`${provider}:${id}`] === "string",
        ),
        "Snapshot credential missing",
      );
      const ids: Record<string, string> = Object.fromEntries(
        references.map(({ provider, id }) => [
          `${provider}:${id}`,
          `executor-sync:${randomUUID()}`,
        ]),
      );
      const imported: Record<string, string> = Object.fromEntries(
        Object.entries(ids).map(([reference, id]) => {
          const secret: string | undefined = snapshot.secrets[reference];
          check(secret !== undefined, "Snapshot credential missing");
          return [id, secret];
        }),
      );
      const context: ImportContext = {
        snapshot,
        ids,
        tenant: tenantFor(this.options.paths.repo),
        paths: this.options.paths,
        now: Date.now(),
      };
      const transaction: Transaction = await client.transaction("write");
      try {
        // Remove derived tool indexes; only settings tables are imported. Other tenants,
        // pending executions, sessions and provider items remain untouched.
        await inOrder(
          [...REBUILD_TABLES, ...TABLES].map(
            (table) => () =>
              transaction.execute({
                sql: `DELETE FROM "${table}" WHERE tenant = ?`,
                args: [context.tenant],
              }),
          ),
        );
        await inOrder(
          TABLES.flatMap((table) =>
            (snapshot.tables[table] ?? []).map((source) => () => {
              const row: PortableRow = remappedRow({
                table,
                row: source,
                context,
              });
              const columns: string[] = snapshot.schema[table] ?? [];
              return transaction.execute({
                sql: `INSERT INTO "${table}" (${columns.map((column) => `"${column}"`).join(",")}) VALUES (${columns.map(() => "?").join(",")})`,
                args: insertValues({ columns, row, context }),
              });
            }),
          ),
        );
        // New IDs never overwrite live credentials. The auth file is committed first;
        // a crash before SQLite commit leaves only harmless unused provider items.
        await atomicWrite(authPath, stable({ ...original, ...imported }));
        await transaction.commit();
      } catch (error: unknown) {
        await transaction.rollback();
        await atomicWrite(authPath, stable(original));
        throw error;
      } finally {
        transaction.close();
      }
    } finally {
      client.close();
    }
  }
}
