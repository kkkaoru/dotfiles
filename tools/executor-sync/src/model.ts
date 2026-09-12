// Runs with Bun. Snapshot validation is fail-closed and never reports secret values.
/** biome-ignore-all lint/suspicious/noTemplateCurlyInString: Portable path tokens are intentionally literal. */
import { createHash } from "node:crypto";
import { z } from "zod";

export interface Paths {
  home: string;
  repo: string;
  commands: Record<string, string>;
}
export interface BinaryCell {
  encoding: "base64";
  data: string;
}
export interface PortableRow {
  [key: string]: string | number | BinaryCell | null;
}
export interface Snapshot {
  format: 1;
  executorVersion: "1.6.8";
  schema: Record<string, string[]>;
  tables: Record<string, PortableRow[]>;
  secrets: Record<string, string>;
}
export interface Envelope {
  group: string;
  device: string;
  revision: string;
  createdAt: string;
  snapshot: Snapshot;
}
export interface SyncState {
  localHash: string | null;
  remoteTag: string | null;
  lastSuccess: string | null;
}
export interface Config {
  format: 1;
  enabled: boolean;
  accountId: string;
  bucket: string;
  group: string;
  intervalSeconds: number;
  ageRecipient: string;
}

export const TABLES: string[] = [
  "integration",
  "connection",
  "oauth_client",
  "tool_policy",
];
export const MAX_BYTES: number = 16 * 1024 * 1024;
export const configSchema = z
  .object({
    format: z.literal(1),
    enabled: z.boolean(),
    accountId: z.union([z.string().regex(/^[a-f0-9]{32}$/), z.literal("")]),
    bucket: z.literal("executor-config-sync"),
    group: z.literal("personal"),
    intervalSeconds: z.literal(30),
    ageRecipient: z.union([
      z.string().regex(/^age1[0-9a-z]{58}$/),
      z.literal(""),
    ]),
  })
  .strict()
  .refine(
    (config) =>
      !config.enabled ||
      (config.accountId !== "" && config.ageRecipient !== ""),
  );
const rowSchema = z.record(
  z.string(),
  z.union([
    z.string(),
    z.number().finite(),
    z.null(),
    z
      .object({
        encoding: z.literal("base64"),
        data: z
          .string()
          .regex(
            /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/,
          ),
      })
      .strict(),
  ]),
);
export const snapshotSchema = z
  .object({
    format: z.literal(1),
    executorVersion: z.literal("1.6.8"),
    schema: z.record(z.string(), z.array(z.string())),
    tables: z.record(z.string(), z.array(rowSchema)),
    secrets: z.record(z.string(), z.string()),
  })
  .strict();
export const envelopeSchema = z
  .object({
    group: z.literal("personal"),
    device: z.string().uuid(),
    revision: z.string().uuid(),
    createdAt: z.string().datetime(),
    snapshot: snapshotSchema,
  })
  .strict();
export const stateSchema = z
  .object({
    localHash: z.string().nullable(),
    remoteTag: z.string().nullable(),
    lastSuccess: z.string().nullable(),
  })
  .strict();
export const INITIAL_STATE: SyncState = {
  localHash: null,
  remoteTag: null,
  lastSuccess: null,
};

export class SafeError extends Error {}
export function check(value: unknown, message: string): asserts value {
  if (!value) throw new SafeError(message);
}
export function parseSnapshot(value: unknown): Snapshot {
  const parsed = snapshotSchema.safeParse(value);
  check(parsed.success, "Invalid snapshot");
  check(
    JSON.stringify(Object.keys(parsed.data.tables).sort()) ===
      JSON.stringify([...TABLES].sort()),
    "Unexpected snapshot tables",
  );
  check(
    JSON.stringify(Object.keys(parsed.data.schema).sort()) ===
      JSON.stringify([...TABLES].sort()),
    "Unexpected snapshot schema",
  );
  return parsed.data;
}
export function parseEnvelope(text: string): Envelope {
  check(Buffer.byteLength(text) <= MAX_BYTES, "Snapshot exceeds size limit");
  const parsed = envelopeSchema.safeParse(JSON.parse(text));
  check(parsed.success, "Invalid encrypted envelope");
  return { ...parsed.data, snapshot: parseSnapshot(parsed.data.snapshot) };
}
export function stable(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.entries(value)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([key, child]) => `${JSON.stringify(key)}:${stable(child)}`)
      .join(",")}}`;
  }
  const encoded: string | undefined = JSON.stringify(value);
  check(encoded !== undefined, "Unsupported snapshot value");
  return encoded;
}
export function fingerprint(snapshot: Snapshot): string {
  return createHash("sha256").update(stable(snapshot)).digest("hex");
}
function normalizeString(value: string, paths: Paths): string {
  const command = Object.entries(paths.commands).find(
    ([, path]) => path === value,
  );
  if (command) return `\${BIN:${command[0]}}`;
  if (value === paths.repo || value.startsWith(`${paths.repo}/`))
    return `\${DOTFILES}${value.slice(paths.repo.length)}`;
  if (value === paths.home || value.startsWith(`${paths.home}/`))
    return `\${HOME}${value.slice(paths.home.length)}`;
  check(
    !value.startsWith("/") && !/=(\/Users\/|\/home\/|\/opt\/)/.test(value),
    "Unmapped absolute path in integration config",
  );
  return value;
}
function expandString(value: string, paths: Paths): string {
  if (value.startsWith("${DOTFILES}")) return `${paths.repo}${value.slice(11)}`;
  if (value.startsWith("${HOME}")) return `${paths.home}${value.slice(7)}`;
  if (value.startsWith("${BIN:")) {
    const command: string | undefined = paths.commands[value.slice(6, -1)];
    check(command && value.endsWith("}"), "Missing local executable mapping");
    return command;
  }
  check(
    !value.startsWith("/") && !value.includes("${"),
    "Unmapped portable path",
  );
  return value;
}
function walk(value: unknown, convert: (text: string) => string): unknown {
  if (typeof value === "string") return convert(value);
  if (Array.isArray(value)) return value.map((child) => walk(child, convert));
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, child]) => [key, walk(child, convert)]),
    );
  return value;
}
export function portableConfig(
  text: string,
  paths: Paths,
  direction: "export" | "import",
): string {
  return stable(
    walk(JSON.parse(text), (value) =>
      direction === "export"
        ? normalizeString(value, paths)
        : expandString(value, paths),
    ),
  );
}
export function tenantFor(repo: string): string {
  return `${repo.split("/").at(-1)}-${createHash("sha256").update(repo).digest("hex").slice(0, 8)}`;
}
