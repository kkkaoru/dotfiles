import { describe, expect, it } from "vitest";
import type { ActiveIdentity } from "./contracts.ts";
import { type LeaseRecord, type LeaseRepository, ProcessDeliveryLease } from "./delivery-lease.ts";

interface MemoryRepository extends LeaseRepository {
  blocked: boolean;
  readonly records: Map<string, LeaseRecord>;
}

const IDENTITY: ActiveIdentity = { agent: "alice", teams: ["one"] };

function createRepository(): MemoryRepository {
  const records = new Map<string, LeaseRecord>();
  return {
    blocked: false,
    read: async (key: string) => records.get(key),
    records,
    remove: async (key: string): Promise<void> => {
      records.delete(key);
    },
    async runExclusive<Value>(
      _key: string,
      operation: () => Promise<Value>,
    ): Promise<Value | undefined> {
      return this.blocked ? undefined : operation();
    },
    write: async (key: string, record: LeaseRecord): Promise<void> => {
      records.set(key, record);
    },
  };
}

function createLease(
  repository: LeaseRepository,
  token: string,
  clock: { readonly value: number },
): ProcessDeliveryLease {
  return new ProcessDeliveryLease({ now: (): number => clock.value, repository, token });
}

describe("ProcessDeliveryLease", () => {
  it("allows only one automatic-delivery owner and transfers after release", async () => {
    const repository = createRepository();
    const clock = { value: 1000 };
    const first = createLease(repository, "first", clock);
    const second = createLease(repository, "second", clock);
    expect(await first.claim({ force: false, identity: IDENTITY })).toBe(true);
    expect(await second.claim({ force: false, identity: IDENTITY })).toBe(false);
    await first.release();
    expect(await second.claim({ force: false, identity: IDENTITY })).toBe(true);
  });

  it("renews its lease and permits takeover after expiration", async () => {
    const repository = createRepository();
    const clock = { value: 1000 };
    const first = createLease(repository, "first", clock);
    const second = createLease(repository, "second", clock);
    await first.claim({ force: false, identity: IDENTITY });
    clock.value = 21_001;
    expect(await second.claim({ force: false, identity: IDENTITY })).toBe(true);
    await first.release();
    expect(repository.records.size).toBe(1);
  });

  it("lets reconnect force ownership away from a live owner", async () => {
    const repository = createRepository();
    const clock = { value: 1000 };
    const first = createLease(repository, "first", clock);
    const second = createLease(repository, "second", clock);
    await first.claim({ force: false, identity: IDENTITY });
    expect(await second.claim({ force: true, identity: IDENTITY })).toBe(true);
    expect(await first.claim({ force: false, identity: IDENTITY })).toBe(false);
  });

  it("releases the previous identity when switching identities", async () => {
    const repository = createRepository();
    const clock = { value: 1000 };
    const first = createLease(repository, "first", clock);
    const second = createLease(repository, "second", clock);
    await first.claim({ force: false, identity: IDENTITY });
    await first.claim({
      force: false,
      identity: { agent: "bob", teams: ["one"] },
    });
    expect(await second.claim({ force: false, identity: IDENTITY })).toBe(true);
  });

  it("releases partial multi-team claims when any mailbox is already owned", async () => {
    const repository = createRepository();
    const clock = { value: 1000 };
    const blocker = createLease(repository, "blocker", clock);
    const contender = createLease(repository, "contender", clock);
    const verifier = createLease(repository, "verifier", clock);
    await blocker.claim({
      force: false,
      identity: { agent: "alice", teams: ["two"] },
    });
    expect(
      await contender.claim({
        force: false,
        identity: { agent: "alice", teams: ["one", "two"] },
      }),
    ).toBe(false);
    expect(
      await verifier.claim({
        force: false,
        identity: { agent: "alice", teams: ["one"] },
      }),
    ).toBe(true);
  });

  it("does not poll when the lease repository is busy", async () => {
    const repository = createRepository();
    repository.blocked = true;
    const lease = createLease(repository, "first", { value: 1000 });
    expect(await lease.claim({ force: false, identity: IDENTITY })).toBe(false);
    await lease.release();
    expect(repository.records.size).toBe(0);
  });
});
