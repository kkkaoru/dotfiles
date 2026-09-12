// Runs with Bun; every external operation is mocked.
import { expect, it, vi } from "vitest";
import { type SyncPorts, syncOnce } from "./engine";
import { envelope, snapshot } from "./fixtures";
import { fingerprint, INITIAL_STATE } from "./model";

function ports(): SyncPorts {
  return {
    exportLocal: vi.fn().mockResolvedValue(snapshot()),
    importLocal: vi.fn().mockResolvedValue(undefined),
    readRemote: vi.fn().mockResolvedValue(null),
    publish: vi.fn().mockResolvedValue("new-tag"),
    saveState: vi.fn().mockResolvedValue(undefined),
    now: () => "2026-01-01T00:00:00.000Z",
  };
}
it("uploads only on an explicitly empty remote", async () => {
  const fake: SyncPorts = ports();
  expect((await syncOnce(fake, INITIAL_STATE)).action).toBe("uploaded");
  expect(fake.importLocal).not.toHaveBeenCalled();
  expect(fake.saveState).toHaveBeenCalledTimes(1);
});
it("does not treat failed reads as absence", async () => {
  const fake: SyncPorts = ports();
  vi.mocked(fake.readRemote).mockRejectedValue(new Error("offline"));
  await expect(syncOnce(fake, INITIAL_STATE)).rejects.toThrow("offline");
  expect(fake.publish).not.toHaveBeenCalled();
  expect(fake.importLocal).not.toHaveBeenCalled();
  expect(fake.saveState).not.toHaveBeenCalled();
});
it("uses remote settings on first sync and does not echo the import", async () => {
  const fake: SyncPorts = ports();
  vi.mocked(fake.readRemote).mockResolvedValue({
    tag: "remote",
    envelope: {
      ...envelope(),
      snapshot: { ...snapshot(), secrets: { remote: "OTHER-TEST" } },
    },
  });
  expect((await syncOnce(fake, INITIAL_STATE)).action).toBe("downloaded");
  expect(fake.importLocal).toHaveBeenCalledTimes(1);
  expect(fake.exportLocal).toHaveBeenCalledTimes(2);
  expect(fake.publish).not.toHaveBeenCalled();
});
it("does not restart when identical data has a different ETag", async () => {
  const fake: SyncPorts = ports();
  vi.mocked(fake.readRemote).mockResolvedValue({
    tag: "remote",
    envelope: envelope(),
  });
  expect((await syncOnce(fake, INITIAL_STATE)).action).toBe("downloaded");
  expect(fake.importLocal).not.toHaveBeenCalled();
});
it("leaves unchanged settings alone", async () => {
  const fake: SyncPorts = ports();
  vi.mocked(fake.readRemote).mockResolvedValue({
    tag: "remote",
    envelope: envelope(),
  });
  expect(
    (
      await syncOnce(fake, {
        localHash: fingerprint(snapshot()),
        remoteTag: "remote",
        lastSuccess: null,
      })
    ).action,
  ).toBe("unchanged");
  expect(fake.publish).not.toHaveBeenCalled();
  expect(fake.importLocal).not.toHaveBeenCalled();
});
it("publishes local edits even if remote also changed (last completed write wins)", async () => {
  const fake: SyncPorts = ports();
  vi.mocked(fake.readRemote).mockResolvedValue({
    tag: "remote",
    envelope: envelope(),
  });
  expect(
    (
      await syncOnce(fake, {
        localHash: "old",
        remoteTag: "previous",
        lastSuccess: null,
      })
    ).action,
  ).toBe("uploaded");
  expect(fake.importLocal).not.toHaveBeenCalled();
});
it("does not acknowledge failed imports", async () => {
  const fake: SyncPorts = ports();
  vi.mocked(fake.readRemote).mockResolvedValue({
    tag: "remote",
    envelope: { ...envelope(), snapshot: { ...snapshot(), secrets: {} } },
  });
  vi.mocked(fake.importLocal).mockRejectedValue(new Error("schema"));
  await expect(syncOnce(fake, INITIAL_STATE)).rejects.toThrow("schema");
  expect(fake.saveState).not.toHaveBeenCalled();
});
it("does not acknowledge failed uploads", async () => {
  const fake: SyncPorts = ports();
  vi.mocked(fake.publish).mockRejectedValue(new Error("offline"));
  await expect(syncOnce(fake, INITIAL_STATE)).rejects.toThrow("offline");
  expect(fake.saveState).not.toHaveBeenCalled();
});
it("does not report success if state cannot be saved", async () => {
  const fake: SyncPorts = ports();
  vi.mocked(fake.saveState).mockRejectedValue(new Error("disk"));
  await expect(syncOnce(fake, INITIAL_STATE)).rejects.toThrow("disk");
});
