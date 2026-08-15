import { createHash, randomUUID } from "node:crypto";
import process from "node:process";
import type { ActiveIdentity, DeliveryLease, DeliveryLeaseRequest } from "./contracts.ts";
import { FileLeaseRepository } from "./file-lease-repository.ts";

const LEASE_DURATION_MS = 20_000 satisfies number;

export interface LeaseRecord {
  readonly expiresAt: number;
  readonly token: string;
}

export interface LeaseRepository {
  readonly read: (key: string) => Promise<LeaseRecord | undefined>;
  readonly remove: (key: string) => Promise<void>;
  readonly runExclusive: <Value>(
    key: string,
    operation: () => Promise<Value>,
  ) => Promise<Value | undefined>;
  readonly write: (key: string, record: LeaseRecord) => Promise<void>;
}

export interface ProcessDeliveryLeaseDependencies {
  readonly now: () => number;
  readonly repository: LeaseRepository;
  readonly token: string;
}

interface ClaimProgress {
  readonly acquired: readonly string[];
  readonly complete: boolean;
}

interface ClaimKeysRequest {
  readonly acquired: readonly string[];
  readonly force: boolean;
  readonly keys: readonly string[];
}

function compareKeys(first: string, second: string): number {
  return first.localeCompare(second);
}

function leaseKeys(identity: ActiveIdentity): readonly string[] {
  return [...new Set(identity.teams)]
    .toSorted(compareKeys)
    .map((team: string): string =>
      createHash("sha256").update(`${team}\u0000${identity.agent}`).digest("hex"),
    );
}

function sameKeys(first: readonly string[], second: readonly string[]): boolean {
  return (
    first.length === second.length &&
    first.every((key: string, index: number) => key === second[index])
  );
}

export class ProcessDeliveryLease implements DeliveryLease {
  readonly #now: () => number;
  readonly #repository: LeaseRepository;
  readonly #token: string;
  #keys: readonly string[] = [];

  constructor({ now, repository, token }: ProcessDeliveryLeaseDependencies) {
    this.#now = now;
    this.#repository = repository;
    this.#token = token;
  }

  static fromSystem(): ProcessDeliveryLease {
    return new ProcessDeliveryLease({
      now: Date.now,
      repository: FileLeaseRepository.fromSystem(),
      token: `${process.pid}-${randomUUID()}`,
    });
  }

  async claim(request: DeliveryLeaseRequest): Promise<boolean> {
    const keys: readonly string[] = leaseKeys(request.identity);
    if (this.#keys.length > 0 && !sameKeys(this.#keys, keys)) {
      await this.release();
    }
    const progress: ClaimProgress = await this.#claimKeys({
      acquired: [],
      force: request.force,
      keys,
    });
    if (!progress.complete) {
      await this.#releaseKeys(progress.acquired);
      this.#keys = [];
      return false;
    }
    this.#keys = keys;
    return true;
  }

  async release(): Promise<void> {
    const keys: readonly string[] = this.#keys;
    this.#keys = [];
    await this.#releaseKeys(keys);
  }

  async #claimKeys(request: ClaimKeysRequest): Promise<ClaimProgress> {
    const key: string | undefined = request.keys.at(request.acquired.length);
    if (key === undefined) {
      return { acquired: request.acquired, complete: true };
    }
    if (!(await this.#claimKey(key, request.force))) {
      return { acquired: request.acquired, complete: false };
    }
    return this.#claimKeys({
      acquired: [...request.acquired, key],
      force: request.force,
      keys: request.keys,
    });
  }

  async #claimKey(key: string, force: boolean): Promise<boolean> {
    const claimed: boolean | undefined = await this.#repository.runExclusive(
      key,
      async (): Promise<boolean> => this.#claimLocked(key, force),
    );
    return claimed === true;
  }

  async #claimLocked(key: string, force: boolean): Promise<boolean> {
    const current: LeaseRecord | undefined = await this.#repository.read(key);
    const now: number = this.#now();
    if (
      current !== undefined &&
      current.token !== this.#token &&
      current.expiresAt > now &&
      !force
    ) {
      return false;
    }
    await this.#repository.write(key, { expiresAt: now + LEASE_DURATION_MS, token: this.#token });
    return true;
  }

  async #releaseKeys(keys: readonly string[]): Promise<void> {
    await Promise.all(
      keys.map(async (key: string): Promise<void> => {
        await this.#repository.runExclusive(key, async (): Promise<void> => {
          const current: LeaseRecord | undefined = await this.#repository.read(key);
          if (current?.token === this.#token) {
            await this.#repository.remove(key);
          }
        });
      }),
    );
  }
}
