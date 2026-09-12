// Runs with Bun. R2 receives age ciphertext only; no plaintext snapshots on disk.
import { randomUUID } from "node:crypto";
import {
  GetObjectCommand,
  ListObjectsV2Command,
  PutObjectCommand,
  S3Client,
} from "@aws-sdk/client-s3";
import { Entry } from "@napi-rs/keyring";
import { z } from "zod";
import type { RemoteValue } from "./engine";
import { type CommandResult, command } from "./io";
import {
  type Config,
  check,
  type Envelope,
  MAX_BYTES,
  parseEnvelope,
  type Snapshot,
  stable,
} from "./model";

export interface Credentials {
  accessKeyId: string;
  secretAccessKey: string;
  identity: string;
}
export interface StorageOptions {
  config: Config;
  credentials: Credentials;
  device: string;
  age: string;
}
export const credentialsSchema = z
  .object({
    accessKeyId: z.string().min(16),
    secretAccessKey: z.string().min(32),
    identity: z.string().regex(/^AGE-SECRET-KEY-1[0-9A-Z]+$/),
  })
  .strict();
export const KEYCHAIN_SERVICE: string = "dotfiles.executor-sync";
export const KEYCHAIN_ACCOUNT: string = "personal";
export function readCredentials(): Credentials {
  const value: string | null = new Entry(
    KEYCHAIN_SERVICE,
    KEYCHAIN_ACCOUNT,
  ).getPassword();
  check(value, "Sync credentials missing from Keychain");
  const parsed = credentialsSchema.safeParse(JSON.parse(value));
  check(parsed.success, "Invalid Keychain sync credentials");
  return parsed.data;
}
export function saveCredentials(credentials: Credentials): void {
  new Entry(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).setPassword(
    JSON.stringify(credentials),
  );
}
export class Storage {
  readonly options: StorageOptions;
  readonly client: S3Client;
  readonly prefix: string;
  constructor(options: StorageOptions) {
    this.options = options;
    this.prefix = `v1/${options.config.group}/`;
    this.client = new S3Client({
      region: "auto",
      endpoint: `https://${options.config.accountId}.r2.cloudflarestorage.com`,
      credentials: {
        accessKeyId: options.credentials.accessKeyId,
        secretAccessKey: options.credentials.secretAccessKey,
      },
      maxAttempts: 2,
    });
  }
  async encrypt(envelope: Envelope): Promise<Buffer> {
    const text: string = stable(envelope);
    check(
      Buffer.byteLength(text) <= MAX_BYTES - 4096,
      "Snapshot exceeds encrypted size limit",
    );
    const result: CommandResult = await command({
      executable: this.options.age,
      args: ["-r", this.options.config.ageRecipient],
      input: text,
      identity: null,
    });
    check(result.code === 0, "Snapshot encryption failed");
    return result.output;
  }
  async decrypt(bytes: Uint8Array): Promise<Envelope> {
    check(
      bytes.byteLength <= MAX_BYTES,
      "Encrypted snapshot exceeds size limit",
    );
    const result: CommandResult = await command({
      executable: this.options.age,
      args: ["-d", "-i", "/dev/fd/3"],
      input: bytes,
      identity: `${this.options.credentials.identity}\n`,
    });
    check(result.code === 0, "Snapshot decryption failed");
    const envelope: Envelope = parseEnvelope(result.output.toString("utf8"));
    check(
      envelope.group === this.options.config.group,
      "Snapshot group mismatch",
    );
    return envelope;
  }
  envelope(snapshot: Snapshot): Envelope {
    return {
      group: this.options.config.group,
      device: this.options.device,
      revision: randomUUID(),
      createdAt: new Date().toISOString(),
      snapshot,
    };
  }
  async read(key: string): Promise<RemoteValue | null> {
    try {
      const response = await this.client.send(
        new GetObjectCommand({
          Bucket: this.options.config.bucket,
          Key: `${this.prefix}${key}`,
        }),
        { abortSignal: AbortSignal.timeout(20_000) },
      );
      check(
        response.Body &&
          response.ETag &&
          response.ContentLength !== undefined &&
          response.ContentLength <= MAX_BYTES,
        "Invalid R2 object",
      );
      return {
        tag: response.ETag,
        envelope: await this.decrypt(
          await response.Body.transformToByteArray(),
        ),
      };
    } catch (error: unknown) {
      if (error instanceof Error && error.name === "NoSuchKey") return null;
      throw new Error("R2 read or snapshot validation failed");
    }
  }
  async put(key: string, bytes: Buffer): Promise<string> {
    const result = await this.client.send(
      new PutObjectCommand({
        Bucket: this.options.config.bucket,
        Key: `${this.prefix}${key}`,
        Body: bytes,
        ContentType: "application/octet-stream",
      }),
      { abortSignal: AbortSignal.timeout(20_000) },
    );
    check(result.ETag, "R2 write returned no ETag");
    return result.ETag;
  }
  async publish(snapshot: Snapshot): Promise<string> {
    const envelope: Envelope = this.envelope(snapshot);
    const bytes: Buffer = await this.encrypt(envelope);
    await this.put(
      `history/${envelope.createdAt.replaceAll(":", "-")}-${envelope.device}-${envelope.revision}.age`,
      bytes,
    );
    return this.put("latest.age", bytes);
  }
  async history(): Promise<string[]> {
    const response = await this.client.send(
      new ListObjectsV2Command({
        Bucket: this.options.config.bucket,
        Prefix: `${this.prefix}history/`,
        MaxKeys: 1000,
      }),
      { abortSignal: AbortSignal.timeout(20_000) },
    );
    check(
      !response.IsTruncated,
      "History exceeds 1000 objects; use R2 console to select an older version",
    );
    return (response.Contents ?? []).flatMap((entry) =>
      entry.Key ? [entry.Key.slice(this.prefix.length)] : [],
    );
  }
}
