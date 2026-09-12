// Runs with Bun. Private destinations, shared authentication keys and enablement stay in Keychain.
import { randomBytes } from "node:crypto";
import { Entry } from "@napi-rs/keyring";
import { z } from "zod";
import { command, requiredCommand } from "./io";
import { type Config, check, configSchema } from "./model";

export interface Profile {
  version: 2;
  config: Config;
  identity: string;
  authKey: string;
  trustedAfter: string | null;
}
export interface PairingOutput {
  profile: Profile;
  target: string;
  age: string;
}
export interface PairingInput {
  bytes: Uint8Array;
  identity: string;
  age: string;
}
export const profileSchema = z
  .object({
    version: z.literal(2),
    config: configSchema,
    identity: z.string().regex(/^AGE-SECRET-KEY-1[0-9A-Z]+$/),
    authKey: z.string().regex(/^[a-f0-9]{64}$/),
    trustedAfter: z.string().datetime().nullable(),
  })
  .strict();
const SERVICE: string = "dotfiles.executor-sync";
const ACCOUNT: string = "profile-v2";
const DEVICE: string = "pairing-device-v2";
export const PUBLIC_CONFIG: string = '{"format":2}\n';

export function readProfile(): Profile {
  const value: string | null = new Entry(SERVICE, ACCOUNT).getPassword();
  check(
    value,
    "Private sync profile missing; initialize or import pairing first",
  );
  return parseProfile(value);
}
export function parseProfile(text: string): Profile {
  const parsed = profileSchema.safeParse(JSON.parse(text));
  check(parsed.success, "Invalid private sync profile");
  check(
    parsed.data.config.accountId && parsed.data.config.ageRecipient,
    "Incomplete private sync profile",
  );
  return parsed.data;
}
export function saveProfile(profile: Profile): void {
  const entry: Entry = new Entry(SERVICE, ACCOUNT);
  const previous: string | null = entry.getPassword();
  const parsed: Profile = parseProfile(JSON.stringify(profile));
  const old: Profile | null = previous === null ? null : parseProfile(previous);
  check(
    !old ||
      (old.authKey === parsed.authKey &&
        old.identity === parsed.identity &&
        old.config.accountId === parsed.config.accountId &&
        old.config.bucket === parsed.config.bucket &&
        old.config.ageRecipient === parsed.config.ageRecipient),
    "Refusing to replace an existing trust identity or destination",
  );
  const trusted: string | null = old?.trustedAfter ?? null;
  entry.setPassword(
    JSON.stringify({
      ...parsed,
      trustedAfter:
        trusted && (!parsed.trustedAfter || trusted > parsed.trustedAfter)
          ? trusted
          : parsed.trustedAfter,
    }),
  );
}
export function requireUnconfigured(): void {
  check(
    new Entry(SERVICE, ACCOUNT).getPassword() === null,
    "A private profile already exists; refusing to replace shared keys",
  );
}
export async function generateIdentity(keygen: string): Promise<string> {
  const output: Buffer = await requiredCommand({
    executable: keygen,
    args: [],
    input: "",
  });
  const identity: string | undefined = output
    .toString()
    .split("\n")
    .find((line) => line.startsWith("AGE-SECRET-KEY-1"));
  check(identity, "age identity generation failed");
  return identity;
}
export async function recipient(
  identity: string,
  keygen: string,
): Promise<string> {
  return (
    await requiredCommand({
      executable: keygen,
      args: ["-y"],
      input: `${identity}\n`,
    })
  )
    .toString()
    .trim();
}
export async function initialize(
  accountId: string,
  keygen: string,
): Promise<void> {
  requireUnconfigured();
  check(/^[a-f0-9]{32}$/.test(accountId), "Invalid account ID");
  const identity: string = await generateIdentity(keygen);
  saveProfile({
    version: 2,
    identity,
    authKey: randomBytes(32).toString("hex"),
    trustedAfter: null,
    config: {
      format: 1,
      enabled: false,
      accountId,
      bucket: "executor-config-sync",
      group: "personal",
      intervalSeconds: 30,
      ageRecipient: await recipient(identity, keygen),
    },
  });
}
export async function pairingRecipient(keygen: string): Promise<string> {
  const entry: Entry = new Entry(SERVICE, DEVICE);
  const identity: string =
    entry.getPassword() ?? (await generateIdentity(keygen));
  entry.setPassword(identity);
  return recipient(identity, keygen);
}
export async function exportPairing(
  target: string,
  age: string,
): Promise<Buffer> {
  return createPairing({ profile: readProfile(), target, age });
}
export async function createPairing(options: PairingOutput): Promise<Buffer> {
  check(/^age1[0-9a-z]{58}$/.test(options.target), "Invalid pairing recipient");
  const result = await command({
    executable: options.age,
    args: ["-r", options.target],
    input: JSON.stringify({
      ...options.profile,
      config: { ...options.profile.config, enabled: false },
    }),
    identity: null,
  });
  check(result.code === 0, "Pairing encryption failed");
  return result.output;
}
export async function importPairing(
  bytes: Uint8Array,
  age: string,
): Promise<void> {
  requireUnconfigured();
  check(bytes.byteLength <= 16384, "Pairing bundle too large");
  const identity: string | null = new Entry(SERVICE, DEVICE).getPassword();
  check(identity, "Run pair-init on this Mac first");
  saveProfile(await openPairing({ bytes, identity, age }));
}
export async function openPairing(options: PairingInput): Promise<Profile> {
  check(options.bytes.byteLength <= 16384, "Pairing bundle too large");
  const result = await command({
    executable: options.age,
    args: ["-d", "-i", "/dev/fd/3"],
    input: options.bytes,
    identity: `${options.identity}\n`,
  });
  check(result.code === 0, "Pairing decryption failed");
  const profile: Profile = parseProfile(result.output.toString());
  return { ...profile, config: { ...profile.config, enabled: false } };
}
