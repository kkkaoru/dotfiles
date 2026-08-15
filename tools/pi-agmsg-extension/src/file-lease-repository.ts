import { randomUUID } from "node:crypto";
import { mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import type { LeaseRecord, LeaseRepository } from "./delivery-lease.ts";

const LOCK_STALE_MS = 5000 satisfies number;
const LEASE_ROOT = path.join(tmpdir(), "pi-agmsg-extension", "delivery-leases") satisfies string;

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null;
}

function decodeLease(value: unknown): LeaseRecord | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const expiresAt: unknown = value["expiresAt"];
  const token: unknown = value["token"];
  return typeof expiresAt === "number" && typeof token === "string"
    ? { expiresAt, token }
    : undefined;
}

function isNodeError(value: unknown): value is NodeJS.ErrnoException {
  return value instanceof Error && "code" in value;
}

export class FileLeaseRepository implements LeaseRepository {
  readonly #root: string;

  constructor(root: string) {
    this.#root = root;
  }

  static fromSystem(): FileLeaseRepository {
    return new FileLeaseRepository(LEASE_ROOT);
  }

  async read(key: string): Promise<LeaseRecord | undefined> {
    try {
      const value: unknown = JSON.parse(await readFile(this.#leasePath(key), "utf8"));
      return decodeLease(value);
    } catch (error: unknown) {
      if (isNodeError(error) && error.code === "ENOENT") {
        return undefined;
      }
      throw error;
    }
  }

  async remove(key: string): Promise<void> {
    await rm(this.#leasePath(key), { force: true });
  }

  async runExclusive<Value>(
    key: string,
    operation: () => Promise<Value>,
  ): Promise<Value | undefined> {
    await mkdir(this.#root, { recursive: true });
    const lockPath = this.#lockPath(key);
    if (!(await this.#acquire(lockPath))) {
      return undefined;
    }
    try {
      return await operation();
    } finally {
      await rm(lockPath, { force: true, recursive: true });
    }
  }

  async write(key: string, record: LeaseRecord): Promise<void> {
    const target = this.#leasePath(key);
    const temporary = `${target}.${process.pid}-${randomUUID()}.tmp`;
    await writeFile(temporary, JSON.stringify(record), { encoding: "utf8", mode: 0o600 });
    await rename(temporary, target);
  }

  async #acquire(lockPath: string): Promise<boolean> {
    try {
      await mkdir(lockPath);
      return true;
    } catch (error: unknown) {
      if (!isNodeError(error) || error.code !== "EEXIST") {
        throw error;
      }
    }
    if (!(await this.#isStale(lockPath))) {
      return false;
    }
    await rm(lockPath, { force: true, recursive: true });
    try {
      await mkdir(lockPath);
      return true;
    } catch (error: unknown) {
      if (isNodeError(error) && error.code === "EEXIST") {
        return false;
      }
      throw error;
    }
  }

  async #isStale(lockPath: string): Promise<boolean> {
    try {
      const metadata: Awaited<ReturnType<typeof stat>> = await stat(lockPath);
      return Date.now() - metadata.mtimeMs > LOCK_STALE_MS;
    } catch (error: unknown) {
      if (isNodeError(error) && error.code === "ENOENT") {
        return true;
      }
      throw error;
    }
  }

  #leasePath(key: string): string {
    return path.join(this.#root, `${key}.json`);
  }

  #lockPath(key: string): string {
    return path.join(this.#root, `${key}.lock`);
  }
}
