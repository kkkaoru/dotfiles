// Runs with Bun. Real age roundtrips use synthetic data; no Keychain, network or real DB access.
import { randomBytes } from "node:crypto";
import { envelope } from "./fixtures";
import { McpStorage } from "./mcp-storage";
import { check, fingerprint } from "./model";
import {
  createPairing,
  generateIdentity,
  openPairing,
  type Profile,
  recipient,
} from "./vault";

async function smoke(): Promise<void> {
  const age: string | null = Bun.which("age");
  const keygen: string | null = Bun.which("age-keygen");
  check(age && keygen, "age and age-keygen required");
  const shared: string = await generateIdentity(keygen);
  const device: string = await generateIdentity(keygen);
  const profile: Profile = {
    version: 2,
    identity: shared,
    authKey: randomBytes(32).toString("hex"),
    trustedAfter: null,
    config: {
      format: 1,
      enabled: true,
      accountId: "a".repeat(32),
      bucket: "executor-config-sync",
      group: "personal",
      intervalSeconds: 30,
      ageRecipient: await recipient(shared, keygen),
    },
  };
  const encrypted: Buffer = await createPairing({
    profile,
    target: await recipient(device, keygen),
    age,
  });
  check(
    !encrypted.toString().includes(shared),
    "Pairing exposed plaintext identity",
  );
  const imported: Profile = await openPairing({
    bytes: encrypted,
    identity: device,
    age,
  });
  check(
    imported.identity === shared &&
      imported.authKey === profile.authKey &&
      !imported.config.enabled,
    "Pairing mismatch",
  );
  await openPairing({ bytes: encrypted, identity: shared, age }).then(
    () => {
      throw new Error("Wrong device decrypted pairing");
    },
    () => undefined,
  );
  const sender: McpStorage = new McpStorage({
    profile,
    age,
    device: "11111111-1111-4111-8111-111111111111",
    executor: "UNUSED",
  });
  const receiver: McpStorage = new McpStorage({
    profile: imported,
    age,
    device: "22222222-2222-4222-8222-222222222222",
    executor: "UNUSED",
  });
  const ciphertext: Buffer = await sender.encrypt(envelope());
  check(
    fingerprint((await receiver.decrypt(ciphertext)).snapshot) ===
      fingerprint(envelope().snapshot),
    "Snapshot transfer mismatch",
  );
  receiver.options.profile.authKey = randomBytes(32).toString("hex");
  await receiver.decrypt(ciphertext).then(
    () => {
      throw new Error("Forged sender accepted");
    },
    () => undefined,
  );
  console.log(
    "PASS: real age pairing to a second device key, wrong-device rejection, authenticated snapshot transfer and wrong-MAC rejection. Synthetic data only.",
  );
}
smoke()
  .then(() => process.exit(0))
  .catch(() => {
    console.error("Secure smoke failed; diagnostics suppressed.");
    process.exit(1);
  });
