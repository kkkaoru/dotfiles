import type { ActiveIdentity, IdentityStore, RuntimeContext } from "./contracts.ts";

const IDENTITY_ENTRY_TYPE = "agmsg-active-identity" satisfies string;

interface IdentityEntryHost {
  readonly appendEntry: (customType: string, data: unknown) => void;
}

interface StoredIdentityEntry {
  readonly customType: typeof IDENTITY_ENTRY_TYPE;
  readonly data: { readonly identity: ActiveIdentity | null };
  readonly type: "custom";
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null;
}

function isActiveIdentity(value: unknown): value is ActiveIdentity {
  if (!isRecord(value) || typeof value["agent"] !== "string") {
    return false;
  }
  const teams: unknown = value["teams"];
  return Array.isArray(teams) && teams.every((team: unknown): boolean => typeof team === "string");
}

function isStoredIdentityEntry(value: unknown): value is StoredIdentityEntry {
  if (!isRecord(value) || value["type"] !== "custom") {
    return false;
  }
  if (value["customType"] !== IDENTITY_ENTRY_TYPE || !isRecord(value["data"])) {
    return false;
  }
  const identity: unknown = value["data"]["identity"];
  return identity === null || isActiveIdentity(identity);
}

function identitiesEqual(
  first: ActiveIdentity | undefined,
  second: ActiveIdentity | undefined,
): boolean {
  if (first === undefined || second === undefined) {
    return first === second;
  }
  return first.agent === second.agent && first.teams.join("\u0000") === second.teams.join("\u0000");
}

export class SessionIdentityStore implements IdentityStore {
  readonly #host: IdentityEntryHost;
  #current: ActiveIdentity | undefined;

  constructor(host: IdentityEntryHost) {
    this.#host = host;
  }

  load(context: RuntimeContext): ActiveIdentity | undefined {
    const entry: StoredIdentityEntry | undefined = context.sessionManager
      .getBranch()
      .findLast((value: unknown): value is StoredIdentityEntry => isStoredIdentityEntry(value));
    this.#current = entry?.data.identity ?? undefined;
    return this.#current;
  }

  save(identity: ActiveIdentity | undefined): void {
    if (identitiesEqual(this.#current, identity)) {
      return;
    }
    this.#current = identity;
    this.#host.appendEntry(IDENTITY_ENTRY_TYPE, { identity: identity ?? null });
  }
}
