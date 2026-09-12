// Runs with Bun. Keychain and encryption subprocesses are mocked with synthetic keys.
import { beforeEach, expect, it, vi } from "vitest";
import {
  exportPairing,
  generateIdentity,
  importPairing,
  initialize,
  type Profile,
  PUBLIC_CONFIG,
  pairingRecipient,
  parseProfile,
  readProfile,
  requireUnconfigured,
  saveProfile,
} from "./vault";

const mocks = vi.hoisted(() => ({
  entries: new Map<string, string>(),
  command: vi.fn(),
  required: vi.fn(),
}));
vi.mock("@napi-rs/keyring", () => ({
  Entry: class {
    constructor(
      readonly service: string,
      readonly account: string,
    ) {}
    getPassword() {
      return mocks.entries.get(this.account) ?? null;
    }
    setPassword(value: string) {
      mocks.entries.set(this.account, value);
    }
  },
}));
vi.mock("./io", () => ({
  command: mocks.command,
  requiredCommand: mocks.required,
}));
function profile(): Profile {
  return {
    version: 2,
    config: {
      format: 1,
      enabled: true,
      accountId: "a".repeat(32),
      bucket: "executor-config-sync",
      group: "personal",
      intervalSeconds: 30,
      ageRecipient: `age1${"q".repeat(58)}`,
    },
    identity: "AGE-SECRET-KEY-1TEST",
    authKey: "b".repeat(64),
    trustedAfter: null,
  };
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.entries.clear();
  mocks.required.mockResolvedValue(Buffer.from("AGE-SECRET-KEY-1TEST\n"));
  mocks.command.mockResolvedValue({
    code: 0,
    output: Buffer.from("ENCRYPTED"),
  });
});
it("keeps every locator and enablement out of public config", () => {
  expect(JSON.parse(PUBLIC_CONFIG)).toStrictEqual({ format: 2 });
});
it("refuses absent, malformed and incomplete profiles", () => {
  expect(readProfile).toThrow("missing");
  expect(() => parseProfile("{}")).toThrow("Invalid private");
  expect(() =>
    parseProfile(
      JSON.stringify({
        ...profile(),
        config: { ...profile().config, enabled: false, accountId: "" },
      }),
    ),
  ).toThrow("Incomplete");
});
it("round trips a validated Keychain profile and refuses replacement", () => {
  saveProfile(profile());
  expect(readProfile().identity).toBe("AGE-SECRET-KEY-1TEST");
  expect(requireUnconfigured).toThrow("already exists");
});
it("never lowers a saved rollback pin or replaces trust through profile updates", () => {
  saveProfile({ ...profile(), trustedAfter: "2099-01-01T00:00:00.000Z" });
  saveProfile(profile());
  expect(readProfile().trustedAfter).toBe("2099-01-01T00:00:00.000Z");
  expect(() => saveProfile({ ...profile(), authKey: "c".repeat(64) })).toThrow(
    "Refusing to replace",
  );
});
it("generates identity without printing keygen output", async () => {
  expect(await generateIdentity("age-keygen")).toBe("AGE-SECRET-KEY-1TEST");
  mocks.required.mockResolvedValue(Buffer.from("bad"));
  await expect(generateIdentity("age-keygen")).rejects.toThrow(
    "generation failed",
  );
});
it("initializes only once with a valid private target and generated authentication key", async () => {
  mocks.required
    .mockResolvedValueOnce(Buffer.from("AGE-SECRET-KEY-1TEST"))
    .mockResolvedValueOnce(Buffer.from(`age1${"q".repeat(58)}`));
  await initialize("a".repeat(32), "keygen");
  expect(readProfile().config.enabled).toBe(false);
  expect(readProfile().authKey.length).toBe(64);
  await expect(initialize("a".repeat(32), "keygen")).rejects.toThrow(
    "already exists",
  );
});
it("rejects malformed accounts before generating keys", async () => {
  await expect(initialize("not-an-account", "keygen")).rejects.toThrow(
    "Invalid account",
  );
  expect(mocks.required).not.toHaveBeenCalled();
});
it("creates a device key and reuses it", async () => {
  mocks.required
    .mockResolvedValueOnce(Buffer.from("AGE-SECRET-KEY-1TEST"))
    .mockResolvedValue(Buffer.from("public"));
  expect(await pairingRecipient("keygen")).toBe("public");
  expect(await pairingRecipient("keygen")).toBe("public");
  expect(mocks.required).toHaveBeenCalledTimes(3);
});
it("exports only encrypted pairing with destination enablement disabled", async () => {
  saveProfile(profile());
  expect((await exportPairing(`age1${"q".repeat(58)}`, "age")).toString()).toBe(
    "ENCRYPTED",
  );
  expect(
    JSON.parse(mocks.command.mock.calls[0]?.[0].input).config.enabled,
  ).toBe(false);
  await expect(exportPairing("bad", "age")).rejects.toThrow("Invalid pairing");
  mocks.command.mockResolvedValue({ code: 1 });
  await expect(exportPairing(`age1${"q".repeat(58)}`, "age")).rejects.toThrow(
    "encryption failed",
  );
});
it("requires a device private key and bounded input for import", async () => {
  await expect(importPairing(Buffer.alloc(16385), "age")).rejects.toThrow(
    "too large",
  );
  await expect(importPairing(Buffer.from("encrypted"), "age")).rejects.toThrow(
    "pair-init",
  );
});
it("imports encrypted pairing via a private FD and keeps sync disabled", async () => {
  mocks.entries.set("pairing-device-v2", "AGE-SECRET-KEY-1DEVICE");
  mocks.command.mockResolvedValue({
    code: 0,
    output: Buffer.from(JSON.stringify(profile())),
  });
  await importPairing(Buffer.from("encrypted"), "age");
  expect(readProfile().config.enabled).toBe(false);
  expect(mocks.command.mock.calls[0]?.[0].identity).toBe(
    "AGE-SECRET-KEY-1DEVICE\n",
  );
});
it("does not save failed pairing decryption", async () => {
  mocks.entries.set("pairing-device-v2", "AGE-SECRET-KEY-1DEVICE");
  mocks.command.mockResolvedValue({ code: 1 });
  await expect(importPairing(Buffer.from("encrypted"), "age")).rejects.toThrow(
    "decryption failed",
  );
  expect(readProfile).toThrow("missing");
});
