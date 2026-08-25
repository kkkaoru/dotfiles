// This TypeScript file is executed with Bun.
import { Stats } from "node:fs";
import fs from "node:fs/promises";
import { tmpdir } from "node:os";
import { setImmediate } from "node:timers/promises";
import { afterEach, expect, it, vi } from "vitest";
import {
  ARTIFACT_CLEANUP_INTERVAL_MILLISECONDS,
  ARTIFACT_RETENTION_MILLISECONDS,
  ArtifactCleaner,
  type ArtifactCleanupOperations,
  type CleanupScheduler,
  type CleanupTimer,
  removeExpiredArtifacts,
  systemArtifactCleanupOperations,
} from "./cleanup.ts";

afterEach(() => {
  vi.restoreAllMocks();
});

it("removes only completed tmux artifacts older than the retention period", async () => {
  const nowMs = 2_000_000_000;
  const readDirectory = vi
    .fn<ArtifactCleanupOperations["readDirectory"]>()
    .mockResolvedValue([
      "not-a-tmux-job",
      "pi-tmux-1-1",
      "pi-tmux-1-2",
      "pi-tmux-1-3",
      "pi-tmux-1-4",
    ]);
  const readFile = vi
    .fn<ArtifactCleanupOperations["readFile"]>()
    .mockImplementation(async (filePath) => {
      if (filePath === "/tmp/pi-tmux-1-4/exit-status") {
        throw new Error("status unavailable");
      }
      return filePath === "/tmp/pi-tmux-1-3/exit-status" ? "running" : "0\n";
    });
  const statMtime = vi
    .fn<ArtifactCleanupOperations["statMtime"]>()
    .mockImplementation(async (filePath) =>
      filePath === "/tmp/pi-tmux-1-2/exit-status"
        ? nowMs - ARTIFACT_RETENTION_MILLISECONDS + 1
        : nowMs - ARTIFACT_RETENTION_MILLISECONDS,
    );
  const removeDirectory = vi.fn<ArtifactCleanupOperations["removeDirectory"]>();

  await removeExpiredArtifacts({
    nowMs: (): number => nowMs,
    operations: { readDirectory, readFile, removeDirectory, statMtime },
    rootDirectory: "/tmp",
  });

  expect(readDirectory).toHaveBeenCalledWith("/tmp");
  expect(removeDirectory).toHaveBeenCalledOnce();
  expect(removeDirectory).toHaveBeenCalledWith("/tmp/pi-tmux-1-1");
});

it("uses the system filesystem adapter for removal and completion time", async () => {
  const statistics: Stats = Object.create(Stats.prototype);
  Object.defineProperty(statistics, "mtimeMs", { value: 1234 });
  const remove = vi.spyOn(fs, "rm").mockResolvedValue();
  vi.spyOn(fs, "stat").mockResolvedValue(statistics);

  await systemArtifactCleanupOperations.removeDirectory(`${tmpdir()}/pi-tmux-1-1`);
  const completedAtMs = await systemArtifactCleanupOperations.statMtime(
    `${tmpdir()}/pi-tmux-1-1/exit-status`,
  );

  expect(remove).toHaveBeenCalledWith(`${tmpdir()}/pi-tmux-1-1`, {
    force: true,
    recursive: true,
  });
  expect(completedAtMs).toBe(1234);
});

it("ignores an unavailable temporary directory", async () => {
  const removeDirectory = vi.fn<ArtifactCleanupOperations["removeDirectory"]>();
  const operations: ArtifactCleanupOperations = {
    readDirectory: vi.fn().mockRejectedValue(new Error("temporary directory unavailable")),
    readFile: vi.fn(),
    removeDirectory,
    statMtime: vi.fn(),
  };

  await removeExpiredArtifacts({ operations, rootDirectory: "/missing" });

  expect(removeDirectory).not.toHaveBeenCalled();
});

it("runs cleanup immediately and hourly without keeping the process alive", async () => {
  const unref = vi.fn<CleanupTimer["unref"]>();
  const timer: CleanupTimer = { cancel: vi.fn(), unref };
  const clear = vi.fn<CleanupScheduler["clear"]>();
  const schedule = vi.fn<CleanupScheduler["schedule"]>((callback): CleanupTimer => {
    callback();
    return timer;
  });
  const readDirectory = vi.fn<ArtifactCleanupOperations["readDirectory"]>().mockResolvedValue([]);
  const cleaner = new ArtifactCleaner({
    operations: {
      readDirectory,
      readFile: vi.fn(),
      removeDirectory: vi.fn(),
      statMtime: vi.fn(),
    },
    scheduler: { clear, schedule },
  });

  await setImmediate();
  cleaner.stop();

  expect(schedule).toHaveBeenCalledWith(
    expect.any(Function),
    ARTIFACT_CLEANUP_INTERVAL_MILLISECONDS,
  );
  expect(unref).toHaveBeenCalledOnce();
  expect(readDirectory).toHaveBeenCalledTimes(2);
  expect(clear).toHaveBeenCalledWith(timer);
});
