// Runs with Bun. DB, filesystem and Keychain are mocked; values are synthetic.
import { beforeEach, expect, it, vi } from "vitest";
import { Adapter } from "./adapter";
import { PATHS } from "./fixtures";
import type { Snapshot } from "./model";

interface Statement {
  sql: string;
  args?: unknown[];
}
const mocks = vi.hoisted(() => ({
  execute: vi.fn(),
  commit: vi.fn(),
  rollback: vi.fn(),
  close: vi.fn(),
  access: vi.fn(),
  readOptional: vi.fn(),
  atomicWrite: vi.fn(),
  getPassword: vi.fn(),
  rows: {
    provider: "file",
    badValue: false,
    bigint: false,
    foreignPlugin: false,
    malformed: false,
    missing: false,
  },
}));
vi.mock("node:fs/promises", () => ({ access: mocks.access }));
vi.mock("./io", () => ({
  readOptional: mocks.readOptional,
  atomicWrite: mocks.atomicWrite,
}));
vi.mock("@napi-rs/keyring", () => ({
  Entry: class {
    getPassword = mocks.getPassword;
  },
}));
vi.mock("@libsql/client", () => ({
  createClient: () => ({
    execute: mocks.execute,
    close: mocks.close,
    transaction: async () => ({
      execute: mocks.execute,
      commit: mocks.commit,
      rollback: mocks.rollback,
      close: mocks.close,
    }),
  }),
}));
const SCHEMA: Record<string, string[]> = {
  integration: [
    "slug",
    "plugin_id",
    "config",
    "created_at",
    "updated_at",
    "config_revised_at",
    "row_id",
    "tenant",
  ],
  connection: [
    "integration",
    "name",
    "provider",
    "item_ids",
    "refresh_item_id",
    "expires_at",
    "owner",
    "subject",
    "row_id",
    "tenant",
    "credential_write",
    "tools_synced_at",
    "last_health",
  ],
  oauth_client: [
    "slug",
    "client_secret_item_id",
    "owner",
    "subject",
    "created_at",
    "row_id",
    "tenant",
  ],
  tool_policy: [
    "id",
    "pattern",
    "action",
    "position",
    "owner",
    "subject",
    "created_at",
    "row_id",
    "tenant",
  ],
};
function adapter(): Adapter {
  return new Adapter({ dataDir: "/mock/executor", paths: PATHS });
}
function blob(text: string): ArrayBuffer {
  return Uint8Array.from(Buffer.from(text)).buffer;
}
beforeEach(() => {
  vi.clearAllMocks();
  Object.assign(mocks.rows, {
    provider: "file",
    badValue: false,
    bigint: false,
    foreignPlugin: false,
    malformed: false,
    missing: false,
  });
  mocks.access.mockResolvedValue(undefined);
  mocks.readOptional.mockResolvedValue(
    '{"access":"TEST-ACCESS","refresh":"TEST-REFRESH","client":"TEST-CLIENT","unrelated":"KEEP"}',
  );
  mocks.atomicWrite.mockResolvedValue(undefined);
  mocks.getPassword.mockReturnValue("TEST-KEYCHAIN");
  mocks.execute.mockImplementation(async (statement: Statement | string) => {
    const sql: string =
      typeof statement === "string" ? statement : statement.sql;
    if (sql.startsWith("PRAGMA table_info")) {
      const table: string = sql.split('"')[1] ?? "";
      return { rows: (SCHEMA[table] ?? []).map((name) => ({ name })) };
    }
    if (sql.startsWith('SELECT * FROM "integration"'))
      return {
        rows: [
          {
            slug: "demo",
            plugin_id: mocks.rows.foreignPlugin ? "other" : "mcp",
            config: mocks.rows.badValue
              ? true
              : blob('{"command":"/opt/homebrew/bin/bun"}'),
            row_id: "i",
            tenant: "local",
            created_at: 1,
            updated_at: 1,
            config_revised_at: 1,
          },
        ],
      };
    if (sql.startsWith('SELECT * FROM "connection"'))
      return {
        rows: [
          {
            integration: "demo",
            name: "default",
            provider: mocks.rows.missing ? null : mocks.rows.provider,
            item_ids: blob(mocks.rows.malformed ? "[]" : '{"token":"access"}'),
            refresh_item_id: "refresh",
            expires_at: mocks.rows.bigint ? 123n : blob("123"),
            owner: "user",
            subject: "local",
            row_id: "c",
            tenant: "local",
            credential_write: null,
            tools_synced_at: null,
            last_health: null,
          },
        ],
      };
    if (sql.startsWith('SELECT * FROM "oauth_client"'))
      return {
        rows: [
          {
            slug: "demo",
            client_secret_item_id: "client",
            owner: "org",
            subject: "",
            created_at: 1,
            row_id: "o",
            tenant: "local",
          },
        ],
      };
    if (sql.startsWith('SELECT * FROM "tool_policy"'))
      return {
        rows: [
          {
            id: "p",
            pattern: "demo.**",
            action: "allow",
            position: "1",
            owner: "user",
            subject: "local",
            created_at: 1,
            row_id: "p",
            tenant: "local",
          },
        ],
      };
    return { rows: [] };
  });
});
it("exports only selected settings and referenced credentials, with JSON/BLOB handling", async () => {
  const value: Snapshot = await adapter().export();
  expect(Object.keys(value.secrets).sort()).toStrictEqual([
    "file:access",
    "file:client",
    "file:refresh",
  ]);
  expect(value.tables.connection?.[0]?.expires_at).toStrictEqual({
    encoding: "base64",
    data: "MTIz",
  });
  expect(value.tables.integration?.[0]?.tenant).toBeUndefined();
  expect(mocks.commit).toHaveBeenCalledTimes(1);
});
it("reads Keychain connections without enumerating unrelated credentials", async () => {
  mocks.rows.provider = "keychain";
  const value: Snapshot = await adapter().export();
  expect(Object.keys(value.secrets).sort()).toStrictEqual([
    "file:client",
    "keychain:access",
    "keychain:refresh",
  ]);
  expect(mocks.getPassword).toHaveBeenCalledTimes(2);
});
it("supports safe bigint values", async () => {
  mocks.rows.bigint = true;
  expect((await adapter().export()).tables.connection?.[0]?.expires_at).toBe(
    123,
  );
});
it("fails closed for unsupported providers", async () => {
  mocks.rows.provider = "external";
  await expect(adapter().export()).rejects.toThrow(
    "Unsupported credential provider",
  );
});
it("fails closed for unsupported plugins", async () => {
  mocks.rows.foreignPlugin = true;
  await expect(adapter().export()).rejects.toThrow("MVP supports MCP");
});
it("rejects unsupported binary/scalar values", async () => {
  mocks.rows.badValue = true;
  await expect(adapter().export()).rejects.toThrow(
    "Unsupported database value",
  );
});
it("rejects missing required fields", async () => {
  mocks.rows.missing = true;
  await expect(adapter().export()).rejects.toThrow(
    "Missing required settings field",
  );
});
it("rejects malformed credential reference maps", async () => {
  mocks.rows.malformed = true;
  await expect(adapter().export()).rejects.toThrow(
    "Invalid credential reference map",
  );
});
it("fails closed for missing referenced credentials", async () => {
  mocks.readOptional.mockResolvedValue(null);
  await expect(adapter().export()).rejects.toThrow(
    "Referenced credential missing",
  );
});
it("does not create a missing database", async () => {
  mocks.access.mockRejectedValue(new Error("missing"));
  await expect(adapter().export()).rejects.toThrow("missing");
  expect(mocks.execute).not.toHaveBeenCalled();
});
it("backs up before importing with the native owner lock", async () => {
  const value: Snapshot = await adapter().export();
  const backup = vi.fn().mockResolvedValue(undefined);
  await adapter().importStopped(value, backup);
  expect(backup).toHaveBeenCalledTimes(1);
  expect(mocks.atomicWrite).toHaveBeenCalledTimes(1);
  expect(mocks.rollback).not.toHaveBeenCalled();
  expect(
    mocks.execute.mock.calls.some(([sql]) => sql === "BEGIN EXCLUSIVE"),
  ).toBe(true);
});
it("refuses import while another daemon holds the owner lock", async () => {
  const value: Snapshot = await adapter().export();
  mocks.execute.mockRejectedValue(new Error("busy"));
  const backup = vi.fn();
  await expect(adapter().importStopped(value, backup)).rejects.toThrow("busy");
  expect(backup).not.toHaveBeenCalled();
  expect(mocks.atomicWrite).not.toHaveBeenCalled();
});
it("refuses schema mismatch before any writes", async () => {
  const value: Snapshot = await adapter().export();
  await expect(
    adapter().importStopped(
      { ...value, schema: { ...value.schema, integration: [] } },
      async () => undefined,
    ),
  ).rejects.toThrow("Schema mismatch");
  expect(mocks.atomicWrite).not.toHaveBeenCalled();
});
it("rejects unexpected input columns", async () => {
  const value: Snapshot = await adapter().export();
  await expect(
    adapter().importStopped(
      {
        ...value,
        tables: { ...value.tables, tool_policy: [{ extra: "value" }] },
      },
      async () => undefined,
    ),
  ).rejects.toThrow("Unexpected snapshot column");
});
it("rejects unsupported imported plugins", async () => {
  const value: Snapshot = await adapter().export();
  await expect(
    adapter().importStopped(
      {
        ...value,
        tables: {
          ...value.tables,
          integration: [{ slug: "x", plugin_id: "other" }],
        },
      },
      async () => undefined,
    ),
  ).rejects.toThrow("Unsupported integration plugin");
});
it("rejects missing incoming credential values", async () => {
  const value: Snapshot = await adapter().export();
  await expect(
    adapter().importStopped({ ...value, secrets: {} }, async () => undefined),
  ).rejects.toThrow("Snapshot credential missing");
});
it("rolls back the DB and restores original auth on a failed auth write", async () => {
  const value: Snapshot = await adapter().export();
  mocks.atomicWrite
    .mockRejectedValueOnce(new Error("disk full"))
    .mockResolvedValue(undefined);
  await expect(
    adapter().importStopped(value, async () => undefined),
  ).rejects.toThrow("disk full");
  expect(mocks.rollback).toHaveBeenCalledTimes(1);
  expect(mocks.atomicWrite).toHaveBeenCalledTimes(2);
});
