import type { ActiveIdentity, IdentityStore, RuntimeContext } from "./contracts.ts";

const IDENTITY_ENTRY_TYPE = "agmsg-active-identity" satisfies string;
const PENDING_ENTRY_TYPE = "agmsg-pending-inbox" satisfies string;

interface IdentityEntryHost {
  readonly appendEntry: (customType: string, data: unknown) => void;
}

interface DecodedIdentity {
  readonly identity: ActiveIdentity | undefined;
}

interface StoredPendingEntry {
  readonly customType: typeof PENDING_ENTRY_TYPE;
  readonly data: { readonly messages: readonly string[] };
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

function decodeIdentityEntry(value: unknown): DecodedIdentity | undefined {
  if (!isRecord(value) || value["type"] !== "custom") {
    return undefined;
  }
  if (value["customType"] !== IDENTITY_ENTRY_TYPE || !isRecord(value["data"])) {
    return undefined;
  }
  const state: unknown = value["data"]["state"];
  if (state === "cleared") {
    return { identity: undefined };
  }
  const identity: unknown = value["data"]["identity"];
  if (identity === null) {
    return { identity: undefined };
  }
  return isActiveIdentity(identity) ? { identity } : undefined;
}

function isDecodedIdentity(value: DecodedIdentity | undefined): value is DecodedIdentity {
  return value !== undefined;
}

function isStoredPendingEntry(value: unknown): value is StoredPendingEntry {
  if (!isRecord(value) || value["type"] !== "custom") {
    return false;
  }
  if (value["customType"] !== PENDING_ENTRY_TYPE || !isRecord(value["data"])) {
    return false;
  }
  const messages: unknown = value["data"]["messages"];
  return (
    Array.isArray(messages) &&
    messages.every((message: unknown): boolean => typeof message === "string")
  );
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
  #pending: readonly string[] = [];
  #pendingLoaded = false satisfies boolean;

  constructor(host: IdentityEntryHost) {
    this.#host = host;
  }

  load(context: RuntimeContext): ActiveIdentity | undefined {
    const decoded: readonly DecodedIdentity[] = context.sessionManager
      .getEntries()
      .map((value: unknown): DecodedIdentity | undefined => decodeIdentityEntry(value))
      .filter((value: DecodedIdentity | undefined): value is DecodedIdentity =>
        isDecodedIdentity(value),
      );
    this.#current = decoded.at(-1)?.identity;
    return this.#current;
  }

  loadPending(context: RuntimeContext): readonly string[] {
    if (this.#pendingLoaded) {
      return this.#pending;
    }
    const entry: StoredPendingEntry | undefined = context.sessionManager
      .getEntries()
      .findLast((value: unknown): value is StoredPendingEntry => isStoredPendingEntry(value));
    this.#pending = entry?.data.messages ?? [];
    this.#pendingLoaded = true;
    return this.#pending;
  }

  save(identity: ActiveIdentity | undefined): void {
    if (identitiesEqual(this.#current, identity)) {
      return;
    }
    this.#current = identity;
    const data: unknown =
      identity === undefined ? { state: "cleared" } : { identity, state: "selected" };
    this.#host.appendEntry(IDENTITY_ENTRY_TYPE, data);
  }

  savePending(messages: readonly string[]): void {
    if (this.#pending.join("\u0000") === messages.join("\u0000")) {
      return;
    }
    this.#pending = messages;
    this.#host.appendEntry(PENDING_ENTRY_TYPE, { messages });
  }
}
