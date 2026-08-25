// This TypeScript file is executed with Bun.
import { Stats } from "node:fs";
import fs from "node:fs/promises";
import { tmpdir } from "node:os";
import { setImmediate } from "node:timers/promises";
import { afterEach, expect, it, vi } from "vitest";
import {
  ARTIFACT_CLEANUP_INTERVAL_MILLISECONDS,
  ARTIFACT_CLEANUP_STAMP_FILENAME,
  ARTIFACT_RETENTION_MILLISECONDS,
  ArtifactCleaner,
  type ArtifactCleanupOperations,
  type CleanupScheduler,
  type CleanupTimer,
  removeExpiredArtifacts,
  removeExpiredArtifactsIfDue,
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
    operations: { readDirectory, readFile, removeDirectory, statMtime, writeFile: vi.fn() },
    rootDirectory: "/tmp",
  });

  expect(readDirectory).toHaveBeenCalledWith("/tmp");
  expect(removeDirectory).toHaveBeenCalledOnce();
  expect(removeDirectory).toHaveBeenCalledWith("/tmp/pi-tmux-1-1");
  expect(readFile).not.toHaveBeenCalledWith("/tmp/pi-tmux-1-2/exit-status");
});

it("uses the system filesystem adapter for removal and completion time", async () => {
  const statistics: Stats = Object.create(Stats.prototype);
  Object.defineProperty(statistics, "mtimeMs", { value: 1234 });
  const readFile = vi.spyOn(fs, "readFile").mockResolvedValue("0\n");
  const remove = vi.spyOn(fs, "rm").mockResolvedValue();
  const writeFile = vi.spyOn(fs, "writeFile").mockResolvedValue();
  vi.spyOn(fs, "stat").mockResolvedValue(statistics);

  expect(await systemArtifactCleanupOperations.readFile(`${tmpdir()}/exit-status`)).toBe("0\n");
  await systemArtifactCleanupOperations.removeDirectory(`${tmpdir()}/pi-tmux-1-1`);
  const completedAtMs = await systemArtifactCleanupOperations.statMtime(
    `${tmpdir()}/pi-tmux-1-1/exit-status`,
  );

  expect(readFile).toHaveBeenCalledWith(`${tmpdir()}/exit-status`, "utf8");
  expect(remove).toHaveBeenCalledWith(`${tmpdir()}/pi-tmux-1-1`, {
    force: true,
    recursive: true,
  });
  await systemArtifactCleanupOperations.writeFile(`${tmpdir()}/stamp`, "stamp");
  expect(completedAtMs).toBe(1234);
  expect(writeFile).toHaveBeenCalledWith(`${tmpdir()}/stamp`, "stamp");
});

it("ignores an unavailable temporary directory", async () => {
  const removeDirectory = vi.fn<ArtifactCleanupOperations["removeDirectory"]>();
  const operations: ArtifactCleanupOperations = {
    readDirectory: vi.fn().mockRejectedValue(new Error("temporary directory unavailable")),
    readFile: vi.fn(),
    removeDirectory,
    statMtime: vi.fn(),
    writeFile: vi.fn(),
  };

  await removeExpiredArtifacts({ operations, rootDirectory: "/missing" });

  expect(removeDirectory).not.toHaveBeenCalled();
});

it("runs a due cleanup once and updates the shared throttle stamp", async () => {
  const nowMs = 2_000_000_000;
  const readDirectory = vi.fn<ArtifactCleanupOperations["readDirectory"]>().mockResolvedValue([]);
  const writeFile = vi
    .fn<ArtifactCleanupOperations["writeFile"]>()
    .mockRejectedValue(new Error("stamp unavailable"));
  await removeExpiredArtifactsIfDue({
    nowMs: (): number => nowMs,
    operations: {
      readDirectory,
      readFile: vi.fn(),
      removeDirectory: vi.fn(),
      statMtime: vi.fn().mockResolvedValue(nowMs - ARTIFACT_CLEANUP_INTERVAL_MILLISECONDS),
      writeFile,
    },
    rootDirectory: "/tmp",
  });

  expect(readDirectory).toHaveBeenCalledOnce();
  expect(writeFile).toHaveBeenCalledWith(
    `/tmp/${ARTIFACT_CLEANUP_STAMP_FILENAME}`,
    expect.stringMatching(/^1970-/u),
  );
});

it("checks a shared daily stamp without scanning on every reload", async () => {
  const unref = vi.fn<CleanupTimer["unref"]>();
  const timer: CleanupTimer = { cancel: vi.fn(), unref };
  const clear = vi.fn<CleanupScheduler["clear"]>();
  const schedule = vi.fn<CleanupScheduler["schedule"]>((callback): CleanupTimer => {
    callback();
    return timer;
  });
  const readDirectory = vi.fn<ArtifactCleanupOperations["readDirectory"]>().mockResolvedValue([]);
  const cleaner = new ArtifactCleaner({
    nowMs: (): number => 2_000_000_000,
    operations: {
      readDirectory,
      readFile: vi.fn(),
      removeDirectory: vi.fn(),
      statMtime: vi.fn().mockResolvedValue(2_000_000_000),
      writeFile: vi.fn(),
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
  expect(readDirectory).not.toHaveBeenCalled();
  expect(clear).toHaveBeenCalledWith(timer);
});
