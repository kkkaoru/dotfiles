// Runs with Bun. No network, real keys, files or production data are used.
import { beforeEach, expect, it, vi } from "vitest";
import { envelope, snapshot } from "./fixtures";
import type { Command } from "./io";
import { McpStorage } from "./mcp-storage";

const mocks = vi.hoisted(() => ({
  command: vi.fn(),
  required: vi.fn(),
  save: vi.fn(),
}));
vi.mock("./io", () => ({
  command: mocks.command,
  requiredCommand: mocks.required,
}));
vi.mock("./vault", () => ({ saveProfile: mocks.save }));
function storage(): McpStorage {
  return new McpStorage({
    executor: "/repo/scripts/executor",
    age: "age",
    device: "11111111-1111-4111-8111-111111111111",
    profile: {
      version: 2,
      config: {
        format: 1,
        enabled: false,
        accountId: "a".repeat(32),
        bucket: "executor-config-sync",
        group: "personal",
        intervalSeconds: 30,
        ageRecipient: `age1${"q".repeat(58)}`,
      },
      identity: "AGE-SECRET-KEY-1TEST",
      authKey: "b".repeat(64),
      trustedAfter: null,
    },
  });
}
function result(value: unknown): Buffer {
  return Buffer.from(
    JSON.stringify({
      ok: true,
      data: { content: [{ type: "text", text: JSON.stringify(value) }] },
    }),
  );
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.command.mockImplementation(async (input: Command) => ({
    code: 0,
    output: Buffer.from(
      input.args[0] === "-d" ? JSON.stringify(envelope()) : "CIPHER",
    ),
  }));
});
it("uses the Executor tool and rejects pauses, errors and malformed results", async () => {
  mocks.required.mockResolvedValue(result({ test: true }));
  expect(await storage().call("async()=>true")).toStrictEqual({ test: true });
  expect(mocks.required.mock.calls[0]?.[0].args.slice(0, 5)).toStrictEqual([
    "call",
    "cloudflare-api",
    "user",
    "default",
    "execute",
  ]);
  mocks.required.mockResolvedValue(Buffer.from('{"ok":false}'));
  await expect(storage().call("test")).rejects.toThrow("requires approval");
  mocks.required.mockResolvedValue(Buffer.from("Execution paused"));
  await expect(storage().call("test")).rejects.toThrow();
});
it("pins the official MCP endpoint", async () => {
  mocks.required.mockResolvedValue(
    Buffer.from(
      '{"ok":true,"data":{"integration":{"config":{"endpoint":"https://mcp.cloudflare.com/mcp"}}}}',
    ),
  );
  await storage().verifyEndpoint();
  mocks.required.mockResolvedValue(
    Buffer.from(
      '{"ok":true,"data":{"integration":{"config":{"endpoint":"https://evil.example"}}}}',
    ),
  );
  await expect(storage().verifyEndpoint()).rejects.toThrow("official");
});
it("rejects traversal keys", () => {
  expect(() => storage().path("../secrets")).toThrow("Invalid object key");
});
it("encrypts and authenticates, rejecting modified ciphertext before age", async () => {
  const s: McpStorage = storage();
  const bytes: Buffer = await s.encrypt(envelope());
  expect((await s.decrypt(bytes)).snapshot.executorVersion).toBe("1.6.8");
  const object = JSON.parse(bytes.toString());
  object.data = Buffer.from("FORGED").toString("base64");
  mocks.command.mockClear();
  await expect(s.decrypt(Buffer.from(JSON.stringify(object)))).rejects.toThrow(
    "authentication failed",
  );
  expect(mocks.command).not.toHaveBeenCalled();
});
it("binds authentication to the destination and shared key", async () => {
  const s: McpStorage = storage();
  const bytes: Buffer = await s.encrypt(envelope());
  s.options.profile.authKey = "c".repeat(64);
  await expect(s.decrypt(bytes)).rejects.toThrow("authentication failed");
});
it("rejects oversized plaintext and ciphertext", async () => {
  await expect(
    storage().encrypt({
      ...envelope(),
      snapshot: { ...snapshot(), secrets: { large: "x".repeat(65536) } },
    }),
  ).rejects.toThrow("limit");
  await expect(storage().decrypt(Buffer.alloc(100001))).rejects.toThrow(
    "too large",
  );
  await expect(storage().decrypt(Buffer.from("{}"))).rejects.toThrow(
    "Invalid authenticated",
  );
});
it("rejects age failures and mismatched groups", async () => {
  mocks.command.mockResolvedValueOnce({ code: 1, output: Buffer.alloc(0) });
  await expect(storage().encrypt(envelope())).rejects.toThrow(
    "encryption failed",
  );
  const s: McpStorage = storage();
  const bytes: Buffer = await s.encrypt(envelope());
  mocks.command.mockResolvedValueOnce({ code: 1, output: Buffer.alloc(0) });
  await expect(s.decrypt(bytes)).rejects.toThrow("decryption failed");
  s.options.profile.config.group = "different";
  const object = JSON.parse(bytes.toString());
  object.mac = s.mac(object.data);
  await expect(s.decrypt(Buffer.from(JSON.stringify(object)))).rejects.toThrow(
    "group mismatch",
  );
});
it("uses exact listing for absence and fails closed after a trusted latest disappears", async () => {
  mocks.required.mockResolvedValue(result({ items: [], truncated: false }));
  const s: McpStorage = storage();
  expect(await s.read("latest.age")).toBeNull();
  s.options.profile.trustedAfter = "2026-01-01T00:00:00.000Z";
  await expect(s.read("latest.age")).rejects.toThrow("missing");
  expect(await s.read("history/2026-01-01T00-00-00.000Z-1111.age")).toBeNull();
});
it("rejects listing truncation instead of calling it absence", async () => {
  mocks.required.mockResolvedValue(result({ items: [], truncated: true }));
  await expect(storage().read("latest.age")).rejects.toThrow();
});
it("reads authenticated content and pins its timestamp", async () => {
  const s: McpStorage = storage();
  const doc = JSON.parse((await s.encrypt(envelope())).toString());
  mocks.required
    .mockResolvedValueOnce(
      result({ items: [{ key: "v1/personal/latest.age" }], truncated: false }),
    )
    .mockResolvedValueOnce(
      result({ total: doc.data.length, mac: doc.mac, part: doc.data }),
    );
  expect((await s.read("latest.age"))?.envelope.snapshot.executorVersion).toBe(
    "1.6.8",
  );
  expect(mocks.save).toHaveBeenCalledTimes(1);
});
it("reads multiple bounded parts and checks their exact length", async () => {
  mocks.command.mockResolvedValueOnce({
    code: 0,
    output: Buffer.from("x".repeat(5000)),
  });
  const s: McpStorage = storage();
  const doc = JSON.parse((await s.encrypt(envelope())).toString());
  mocks.required
    .mockResolvedValueOnce(
      result({ items: [{ key: "v1/personal/latest.age" }], truncated: false }),
    )
    .mockResolvedValueOnce(
      result({
        total: doc.data.length,
        mac: doc.mac,
        part: doc.data.slice(0, 6000),
      }),
    )
    .mockResolvedValueOnce(
      result({
        total: doc.data.length,
        mac: doc.mac,
        part: doc.data.slice(6000),
      }),
    );
  expect((await s.read("latest.age"))?.envelope.snapshot.executorVersion).toBe(
    "1.6.8",
  );
});
it("refuses changed versions between parts", async () => {
  mocks.required
    .mockResolvedValueOnce(
      result({ items: [{ key: "v1/personal/latest.age" }], truncated: false }),
    )
    .mockResolvedValueOnce(
      result({ total: 6001, mac: "a".repeat(64), part: "x".repeat(6000) }),
    )
    .mockResolvedValueOnce(
      result({ total: 6001, mac: "b".repeat(64), part: "x" }),
    );
  await expect(storage().read("latest.age")).rejects.toThrow(
    "changed during download",
  );
});
it("refuses a shortened last part", async () => {
  mocks.required
    .mockResolvedValueOnce(
      result({ items: [{ key: "v1/personal/latest.age" }], truncated: false }),
    )
    .mockResolvedValueOnce(
      result({ total: 5, mac: "a".repeat(64), part: "x" }),
    );
  await expect(storage().read("latest.age")).rejects.toThrow("Truncated");
});
it("publishes authenticated history before latest and pins after success", async () => {
  mocks.required.mockResolvedValue(result({ success: true, status: 200 }));
  const s: McpStorage = storage();
  expect((await s.publish(snapshot())).length).toBe(64);
  expect(mocks.required).toHaveBeenCalledTimes(2);
  expect(mocks.required.mock.calls[0]?.[0].args[5]).toMatch(/history%2F/);
  expect(mocks.required.mock.calls[1]?.[0].args[5]).toMatch(/latest\.age/);
  expect(mocks.save).toHaveBeenCalledTimes(1);
});
it("does not publish latest after failed history", async () => {
  mocks.required.mockResolvedValue(result({ success: false, status: 403 }));
  await expect(storage().publish(snapshot())).rejects.toThrow("upload failed");
  expect(mocks.required).toHaveBeenCalledTimes(1);
  expect(mocks.save).not.toHaveBeenCalled();
});
it("rejects rollback but permits deliberate history reads without lowering the pin", async () => {
  const s: McpStorage = storage();
  s.options.profile.trustedAfter = "2099-01-01T00:00:00.000Z";
  expect(() => s.pin(envelope())).toThrow("Older remote");
  expect(s.envelope(snapshot()).createdAt).toBe("2099-01-01T00:00:00.001Z");
  const doc = JSON.parse((await s.encrypt(envelope())).toString());
  mocks.required
    .mockResolvedValueOnce(
      result({
        items: [
          { key: "v1/personal/history/2026-01-01T00-00-00.000Z-1111.age" },
        ],
        truncated: false,
      }),
    )
    .mockResolvedValueOnce(
      result({ total: doc.data.length, mac: doc.mac, part: doc.data }),
    );
  expect(
    (await s.read("history/2026-01-01T00-00-00.000Z-1111.age"))?.envelope
      .snapshot.executorVersion,
  ).toBe("1.6.8");
  expect(mocks.save).not.toHaveBeenCalled();
});
it("returns bounded history keys", async () => {
  mocks.required.mockResolvedValue(
    result({
      items: [{ key: "v1/personal/history/one.age" }],
      truncated: false,
    }),
  );
  expect(await storage().history()).toStrictEqual(["history/one.age"]);
  storage().client.destroy();
});
