import { beforeEach, describe, expect, it, vi } from "vitest";

const fileSystem = vi.hoisted(() => ({
  mkdir: vi.fn(),
  readFile: vi.fn(),
  rename: vi.fn(),
  rm: vi.fn(),
  stat: vi.fn(),
  writeFile: vi.fn(),
}));

vi.mock("node:fs/promises", () => fileSystem);

import { FileLeaseRepository } from "./file-lease-repository.ts";

function fileError(code: string): NodeJS.ErrnoException {
  return Object.assign(new Error(code), { code });
}

describe("FileLeaseRepository", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    fileSystem.mkdir.mockResolvedValue(undefined);
    fileSystem.rename.mockResolvedValue(undefined);
    fileSystem.rm.mockResolvedValue(undefined);
    fileSystem.stat.mockResolvedValue({ mtimeMs: Date.now() });
    fileSystem.writeFile.mockResolvedValue(undefined);
  });

  it("reads valid leases and treats missing files as absent", async () => {
    const repository = new FileLeaseRepository("/leases");
    fileSystem.readFile.mockResolvedValueOnce('{"expiresAt":42,"token":"owner"}');
    expect(await repository.read("identity")).toStrictEqual({ expiresAt: 42, token: "owner" });
    fileSystem.readFile.mockRejectedValueOnce(fileError("ENOENT"));
    expect(await repository.read("missing")).toBeUndefined();
  });

  it("rejects invalid lease data and unexpected read errors", async () => {
    const repository = new FileLeaseRepository("/leases");
    fileSystem.readFile.mockResolvedValueOnce('{"expiresAt":"later","token":"owner"}');
    expect(await repository.read("invalid")).toBeUndefined();
    fileSystem.readFile.mockRejectedValueOnce(fileError("EACCES"));
    await expect(repository.read("denied")).rejects.toThrow("EACCES");
  });

  it("serializes lease updates with a temporary lock directory", async () => {
    const repository = new FileLeaseRepository("/leases");
    const operation = vi.fn(async () => "updated");
    expect(await repository.runExclusive("identity", operation)).toBe("updated");
    expect(operation).toHaveBeenCalledOnce();
    expect(fileSystem.rm).toHaveBeenCalledWith("/leases/identity.lock", {
      force: true,
      recursive: true,
    });
  });

  it("waits for a fresh lock and reclaims a stale lock", async () => {
    const repository = new FileLeaseRepository("/leases");
    fileSystem.mkdir.mockResolvedValueOnce(undefined).mockRejectedValueOnce(fileError("EEXIST"));
    expect(await repository.runExclusive("busy", async () => "unexpected")).toBeUndefined();

    vi.resetAllMocks();
    fileSystem.mkdir
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(fileError("EEXIST"))
      .mockResolvedValueOnce(undefined);
    fileSystem.stat.mockResolvedValue({ mtimeMs: 0 });
    fileSystem.rm.mockResolvedValue(undefined);
    expect(await repository.runExclusive("stale", async () => "reclaimed")).toBe("reclaimed");
  });

  it("handles lock races and unexpected filesystem errors", async () => {
    const repository = new FileLeaseRepository("/leases");
    fileSystem.mkdir
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(fileError("EEXIST"))
      .mockRejectedValueOnce(fileError("EEXIST"));
    fileSystem.stat.mockRejectedValue(fileError("ENOENT"));
    fileSystem.rm.mockResolvedValue(undefined);
    expect(await repository.runExclusive("raced", async () => "unexpected")).toBeUndefined();

    vi.resetAllMocks();
    fileSystem.mkdir.mockResolvedValueOnce(undefined).mockRejectedValueOnce(fileError("EACCES"));
    await expect(repository.runExclusive("denied", async () => "unexpected")).rejects.toThrow(
      "EACCES",
    );
  });

  it("writes atomically and removes released leases", async () => {
    const repository = new FileLeaseRepository("/leases");
    await repository.write("identity", { expiresAt: 42, token: "owner" });
    expect(fileSystem.writeFile).toHaveBeenCalledWith(
      expect.stringMatching(/^\/leases\/identity\.json\.\d+-.+\.tmp$/u),
      '{"expiresAt":42,"token":"owner"}',
      { encoding: "utf8", mode: 0o600 },
    );
    expect(fileSystem.rename).toHaveBeenCalledWith(
      expect.stringMatching(/^\/leases\/identity\.json\.\d+-.+\.tmp$/u),
      "/leases/identity.json",
    );
    await repository.remove("identity");
    expect(fileSystem.rm).toHaveBeenLastCalledWith("/leases/identity.json", { force: true });
  });
});
