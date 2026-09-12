// Runs with Bun. R2, age and Keychain are mocked; all credentials are synthetic.
import { beforeEach, expect, it, vi } from "vitest";
import { envelope, snapshot } from "./fixtures";
import type { Command } from "./io";
import { MAX_BYTES } from "./model";
import { readCredentials, Storage, saveCredentials } from "./storage";

const mocks = vi.hoisted(() => ({
  send: vi.fn(),
  command: vi.fn(),
  getPassword: vi.fn(),
  setPassword: vi.fn(),
}));
vi.mock("@aws-sdk/client-s3", () => {
  class TestCommand {
    constructor(readonly input: Record<string, unknown>) {}
  }
  return {
    S3Client: class {
      send = mocks.send;
    },
    GetObjectCommand: TestCommand,
    PutObjectCommand: TestCommand,
    ListObjectsV2Command: TestCommand,
  };
});
vi.mock("@napi-rs/keyring", () => ({
  Entry: class {
    getPassword = mocks.getPassword;
    setPassword = mocks.setPassword;
  },
}));
vi.mock("./io", () => ({ command: mocks.command }));
function storage(): Storage {
  return new Storage({
    config: {
      format: 1,
      enabled: true,
      accountId: "a".repeat(32),
      bucket: "executor-config-sync",
      group: "personal",
      intervalSeconds: 30,
      ageRecipient: `age1${"q".repeat(58)}`,
    },
    credentials: {
      accessKeyId: "TEST-ACCESS-KEY-ID",
      secretAccessKey: "TEST-SECRET-ACCESS-KEY-NOT-REAL-000",
      identity: "AGE-SECRET-KEY-1TEST",
    },
    device: "11111111-1111-4111-8111-111111111111",
    age: "/mock/age",
  });
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.command.mockImplementation(async (options: Command) => ({
    code: 0,
    output: Buffer.from(
      options.args[0] === "-d" ? JSON.stringify(envelope()) : "MOCK-CIPHERTEXT",
    ),
  }));
  mocks.send.mockResolvedValue({ ETag: "tag" });
});
it("stores history before latest and uploads ciphertext only", async () => {
  expect(await storage().publish(snapshot())).toBe("tag");
  expect(mocks.send).toHaveBeenCalledTimes(2);
  const first = mocks.send.mock.calls[0]?.[0];
  const last = mocks.send.mock.calls[1]?.[0];
  expect(first.input.Key.startsWith("v1/personal/history/")).toBe(true);
  expect(last.input.Key).toBe("v1/personal/latest.age");
  expect(last.input.Body.toString()).toBe("MOCK-CIPHERTEXT");
});
it("does not publish latest if history upload fails", async () => {
  mocks.send.mockRejectedValue(new Error("offline"));
  await expect(storage().publish(snapshot())).rejects.toThrow("offline");
  expect(mocks.send).toHaveBeenCalledTimes(1);
});
it("reads, bounds and decrypts remote settings", async () => {
  mocks.send.mockResolvedValue({
    ETag: "remote-tag",
    ContentLength: 16,
    Body: { transformToByteArray: async () => Buffer.from("CIPHER") },
  });
  expect((await storage().read("latest.age"))?.tag).toBe("remote-tag");
  expect(mocks.command.mock.calls[0]?.[0].args).toStrictEqual([
    "-d",
    "-i",
    "/dev/fd/3",
  ]);
  expect(mocks.command.mock.calls[0]?.[0].identity).toBe(
    "AGE-SECRET-KEY-1TEST\n",
  );
});
it("treats NoSuchKey as absence", async () => {
  const error: Error = new Error("missing");
  error.name = "NoSuchKey";
  mocks.send.mockRejectedValue(error);
  expect(await storage().read("latest.age")).toBeNull();
});
it("does not treat missing buckets or permission errors as absence", async () => {
  mocks.send.mockRejectedValue(new Error("AccessDenied"));
  await expect(storage().read("latest.age")).rejects.toThrow(
    "R2 read or snapshot validation failed",
  );
});
it("does not decrypt oversized objects", async () => {
  mocks.send.mockResolvedValue({
    ETag: "x",
    ContentLength: MAX_BYTES + 1,
    Body: {},
  });
  await expect(storage().read("latest.age")).rejects.toThrow();
  expect(mocks.command).not.toHaveBeenCalled();
});
it("rejects a response without body or ETag", async () => {
  mocks.send.mockResolvedValue({});
  await expect(storage().read("latest.age")).rejects.toThrow();
});
it("rejects oversized decrypted input before starting age", async () => {
  await expect(storage().decrypt(Buffer.alloc(MAX_BYTES + 1))).rejects.toThrow(
    "Encrypted snapshot exceeds size limit",
  );
  expect(mocks.command).not.toHaveBeenCalled();
});
it("rejects failed encryption", async () => {
  mocks.command.mockResolvedValue({ code: 1, output: Buffer.alloc(0) });
  await expect(storage().encrypt(envelope())).rejects.toThrow(
    "Snapshot encryption failed",
  );
});
it("rejects failed decryption", async () => {
  mocks.command.mockResolvedValue({ code: 1, output: Buffer.alloc(0) });
  await expect(storage().decrypt(Buffer.alloc(0))).rejects.toThrow(
    "Snapshot decryption failed",
  );
});
it("rejects plaintext too large to encrypt", async () => {
  await expect(
    storage().encrypt({
      ...envelope(),
      snapshot: { ...snapshot(), secrets: { big: "x".repeat(MAX_BYTES) } },
    }),
  ).rejects.toThrow("Snapshot exceeds encrypted size limit");
});
it("rejects an upload without an ETag", async () => {
  mocks.send.mockResolvedValue({});
  await expect(
    storage().put("latest.age", Buffer.from("CIPHER")),
  ).rejects.toThrow("R2 write returned no ETag");
});
it("lists bounded history identifiers", async () => {
  mocks.send.mockResolvedValue({
    Contents: [{ Key: "v1/personal/history/one.age" }, {}],
    IsTruncated: false,
  });
  expect(await storage().history()).toStrictEqual(["history/one.age"]);
});
it("handles empty history", async () => {
  mocks.send.mockResolvedValue({});
  expect(await storage().history()).toStrictEqual([]);
});
it("does not silently truncate history", async () => {
  mocks.send.mockResolvedValue({ IsTruncated: true });
  await expect(storage().history()).rejects.toThrow(
    "History exceeds 1000 objects",
  );
});
it("fails closed for missing Keychain data", () => {
  mocks.getPassword.mockReturnValue(null);
  expect(() => readCredentials()).toThrow("Sync credentials missing");
});
it("fails closed for malformed Keychain data", () => {
  mocks.getPassword.mockReturnValue("{}");
  expect(() => readCredentials()).toThrow("Invalid Keychain sync credentials");
});
it("stores and reads credentials through Keychain, never arguments", () => {
  saveCredentials(storage().options.credentials);
  mocks.getPassword.mockReturnValue(
    JSON.stringify(storage().options.credentials),
  );
  expect(readCredentials().accessKeyId).toBe("TEST-ACCESS-KEY-ID");
  expect(mocks.setPassword).toHaveBeenCalledTimes(1);
});
